use super::{Request, Response, SCHEMA_VERSION};
use crate::fleet::{run_db, sql};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_REQUEST_ID_BYTES: usize = 100;

/// The pursuit tables in the fleet database `jeden`, created by
/// `crate::fleet` on connection.
pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS pursuit_values (request TEXT NOT NULL, key TEXT NOT NULL,
        data TEXT NOT NULL, PRIMARY KEY(request,key));
    CREATE TABLE IF NOT EXISTS pursuit_stages (request TEXT NOT NULL, position BIGINT NOT NULL,
        data TEXT NOT NULL, PRIMARY KEY(request,position));
    CREATE TABLE IF NOT EXISTS pursuit_calls (request TEXT NOT NULL, id TEXT NOT NULL,
        model TEXT NOT NULL, catalog_revision TEXT NOT NULL, reserved TEXT NOT NULL, actual TEXT,
        PRIMARY KEY(request,id));";

/// `pursuit_values`: one JSON value of a request, by key.
pub(super) mod value {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "pursuit_values")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub request: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        pub data: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

/// `pursuit_stages`: one completed pursuit stage of a request, by position.
pub(super) mod stage {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "pursuit_stages")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub request: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub position: i64,
        pub data: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

/// One pursuit request: its values and stages live in the fleet database
/// under the request id; `directory` keeps only the run artifacts and the
/// owner lock, which are local to the process that executes it.
pub(super) struct Store {
    pub directory: PathBuf,
    id: String,
}
/// Held while this process executes the request; the kernel drops the lock
/// with the file when the process ends, however it ends.
pub(super) struct Claim {
    _file: fs::File,
}

pub(super) fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_REQUEST_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn root() -> Result<PathBuf, String> {
    let root = std::env::var_os("JEDEN_PURSUIT_STATE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::user_config_path()
                .parent()
                .expect("user config has a parent")
                .join("pursuit")
        });
    if !root.is_absolute() {
        return Err("Jeden pursuit state root must be absolute".into());
    }
    Ok(root)
}
fn directory(id: &str) -> Result<PathBuf, String> {
    if !identifier(id) {
        return Err(
            "pursuit request id must be 1–100 ASCII letters, digits, hyphens or underscores".into(),
        );
    }
    Ok(root()?.join(id))
}
impl Store {
    pub fn open(request: &Request) -> Result<Self, String> {
        let directory = directory(&request.request_id)?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|e| format!("create request state: {e}"))?;
        if fs::symlink_metadata(&directory)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("pursuit state directory must not be a symlink".into());
        }
        let store = Self {
            directory,
            id: request.request_id.clone(),
        };
        let encoded = serde_json::to_string(request).map_err(|e| e.to_string())?;
        let (id, data) = (store.id.clone(), encoded.clone());
        let existing: String = run_db(move |db| async move {
            value::Entity::insert(value::ActiveModel {
                request: Set(id.clone()),
                key: Set("request".into()),
                data: Set(data),
            })
            .on_conflict(
                OnConflict::columns([value::Column::Request, value::Column::Key])
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(&db)
            .await
            .map_err(sql)?;
            value::Entity::find_by_id((id, "request".to_owned()))
                .one(&db)
                .await
                .map_err(sql)?
                .map(|row| row.data)
                .ok_or_else(|| "the request row vanished after it was written".to_owned())
        })?;
        if existing != encoded {
            return Err(
                "request_id_conflict: this request id already names a different immutable payload"
                    .into(),
            );
        }
        if let Some(version) = store.get::<u32>("schema_version")? {
            if version != SCHEMA_VERSION {
                return Err("unsupported pursuit request state schema".into());
            }
        } else {
            store.set("schema_version", &SCHEMA_VERSION)?;
        }
        Ok(store)
    }
    pub fn existing(id: &str) -> Result<Self, String> {
        let directory = directory(id)?;
        let key = (id.to_owned(), "request".to_owned());
        let known = run_db(move |db| async move {
            Ok(value::Entity::find_by_id(key)
                .one(&db)
                .await
                .map_err(sql)?
                .is_some())
        })?;
        if !known {
            return Err(format!(
                "request {id} is unavailable: no such request in the fleet database"
            ));
        }
        Ok(Self {
            directory,
            id: id.to_owned(),
        })
    }
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, String> {
        let key = (self.id.clone(), key.to_owned());
        let text = run_db(move |db| async move {
            Ok(value::Entity::find_by_id(key)
                .one(&db)
                .await
                .map_err(sql)?
                .map(|row| row.data))
        })?;
        text.map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn set(&self, key: &str, value: &impl Serialize) -> Result<(), String> {
        let row = value::ActiveModel {
            request: Set(self.id.clone()),
            key: Set(key.to_owned()),
            data: Set(serde_json::to_string(value).map_err(|e| e.to_string())?),
        };
        run_db(move |db| async move {
            value::Entity::insert(row)
                .on_conflict(
                    OnConflict::columns([value::Column::Request, value::Column::Key])
                        .update_column(value::Column::Data)
                        .to_owned(),
                )
                .exec_without_returning(&db)
                .await
                .map_err(sql)?;
            Ok(())
        })
    }
    pub fn stage<T: DeserializeOwned>(&self, position: usize) -> Result<Option<T>, String> {
        let key = (self.id.clone(), position as i64);
        let text = run_db(move |db| async move {
            Ok(stage::Entity::find_by_id(key)
                .one(&db)
                .await
                .map_err(sql)?
                .map(|row| row.data))
        })?;
        text.map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn record_stage(&self, position: usize, value: &impl Serialize) -> Result<(), String> {
        let row = stage::ActiveModel {
            request: Set(self.id.clone()),
            position: Set(position as i64),
            data: Set(serde_json::to_string(value).map_err(|e| e.to_string())?),
        };
        run_db(move |db| async move {
            stage::Entity::insert(row)
                .on_conflict(
                    OnConflict::columns([stage::Column::Request, stage::Column::Position])
                        .update_column(stage::Column::Data)
                        .to_owned(),
                )
                .exec_without_returning(&db)
                .await
                .map_err(sql)?;
            Ok(())
        })
    }
    /// The request id the fleet database keys this request's rows by.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Every value stored for this request, by key, as `pursue --state` shows.
    pub fn saved(&self) -> Result<std::collections::BTreeMap<String, serde_json::Value>, String> {
        let id = self.id.clone();
        let rows = run_db(move |db| async move {
            value::Entity::find()
                .filter(value::Column::Request.eq(id))
                .order_by_asc(value::Column::Key)
                .all(&db)
                .await
                .map_err(sql)
        })?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str(&row.data)
                    .map(|value| (row.key, value))
                    .map_err(|e| e.to_string())
            })
            .collect()
    }
    pub fn claim(&self) -> Result<Option<Claim>, String> {
        use std::os::unix::io::AsRawFd;
        // A request resumed on another host has no local directory yet.
        fs::create_dir_all(&self.directory).map_err(|e| format!("claim pursuit request: {e}"))?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.directory.join("owner.lock"))
            .map_err(|e| format!("claim pursuit request: {e}"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(Claim { _file: file }));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(None)
        } else {
            Err(format!("claim pursuit request: {error}"))
        }
    }
    pub fn response(&self) -> Result<Response, String> {
        self.get("response")?
            .ok_or_else(|| "request exists but has not recorded an execution response".into())
    }
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}
