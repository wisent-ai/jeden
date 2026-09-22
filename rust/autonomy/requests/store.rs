use super::{Request, Response, SCHEMA_VERSION};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_REQUEST_ID_BYTES: usize = 100;

pub(super) struct Store {
    pub directory: PathBuf,
    connection: Connection,
}
pub(super) struct Claim {
    _connection: Connection,
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
        let connection =
            Connection::open(directory.join("state.sqlite3")).map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS values_store (key TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS stages (position INTEGER PRIMARY KEY, data TEXT NOT NULL);",
            )
            .map_err(|e| e.to_string())?;
        let store = Self {
            directory,
            connection,
        };
        let encoded = serde_json::to_string(request).map_err(|e| e.to_string())?;
        store
            .connection
            .execute(
                "INSERT OR IGNORE INTO values_store VALUES ('request',?1)",
                [&encoded],
            )
            .map_err(|e| e.to_string())?;
        let existing: String = store
            .connection
            .query_row(
                "SELECT data FROM values_store WHERE key='request'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
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
        let connection = Connection::open_with_flags(
            directory.join("state.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .map_err(|e| format!("request {id} is unavailable: {e}"))?;
        Ok(Self {
            directory,
            connection,
        })
    }
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, String> {
        let text: Option<String> = self
            .connection
            .query_row("SELECT data FROM values_store WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        text.map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn set(&self, key: &str, value: &impl Serialize) -> Result<(), String> {
        self.connection.execute("INSERT INTO values_store VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET data=excluded.data",
            params![key,serde_json::to_string(value).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn stage<T: DeserializeOwned>(&self, position: usize) -> Result<Option<T>, String> {
        let text: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM stages WHERE position=?1",
                [position as u64],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        text.map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn record_stage(&self, position: usize, value: &impl Serialize) -> Result<(), String> {
        self.connection.execute("INSERT INTO stages VALUES (?1,?2) ON CONFLICT(position) DO UPDATE SET data=excluded.data",
            params![position as u64,serde_json::to_string(value).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn claim(&self) -> Result<Option<Claim>, String> {
        let connection =
            Connection::open(self.directory.join("owner.sqlite3")).map_err(|e| e.to_string())?;
        match connection.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => Ok(Some(Claim {
                _connection: connection,
            })),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::DatabaseBusy
                    || error.code == rusqlite::ErrorCode::DatabaseLocked =>
            {
                Ok(None)
            }
            Err(error) => Err(format!("claim pursuit request: {error}")),
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
