//! Operator-owned model API keys. SQLite serializes selection across processes.
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenRow {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub active: bool,
    pub last_failure: Option<String>,
}
pub(crate) struct SelectedToken {
    pub id: String,
    pub secret: String,
}
pub(crate) struct Store {
    connection: Connection,
}

pub(crate) fn canonical_endpoint(endpoint: &str) -> Result<String, String> {
    let url =
        url::Url::parse(endpoint).map_err(|error| format!("invalid model endpoint: {error}"))?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name == "localhost",
        None => false,
    };
    let secure = url.scheme() == "https";
    let local = url.scheme() == "http" && loopback;
    let userinfo = endpoint.split_once("://").is_some_and(|(_, rest)| {
        rest.split('/')
            .next()
            .is_some_and(|authority| authority.contains('@'))
    });
    if !(secure || local)
        || userinfo
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "model endpoint must be HTTPS or loopback HTTP, without credentials, query or fragment"
                .into(),
        );
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}
fn path() -> PathBuf {
    crate::dirs_home().join(".jeden/model-tokens.sqlite3")
}

#[cfg(unix)]
fn check_directory(dir: &Path, protect: bool) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = fs::symlink_metadata(dir)
        .map_err(|error| format!("inspect token directory {}: {error}", dir.display()))?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "token directory {} is not owned by this user or is a symlink",
            dir.display()
        ));
    }
    if protect {
        fs::set_permissions(dir, fs::Permissions::from_mode(libc::S_IRWXU as u32))
            .map_err(|error| format!("protect token directory {}: {error}", dir.display()))?;
    } else if std::num::NonZeroU32::new(
        meta.permissions().mode() & ((libc::S_IRWXG | libc::S_IRWXO) as u32),
    )
    .is_some()
    {
        return Err(format!("token directory {} is not private", dir.display()));
    }
    Ok(())
}
fn check_file(path: &Path, create: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        if create {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode((libc::S_IRUSR | libc::S_IWUSR) as u32)
                .open(path)
            {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!("create token store {}: {error}", path.display()))
                }
            }
        }
        let meta = fs::symlink_metadata(path)
            .map_err(|error| format!("inspect token store {}: {error}", path.display()))?;
        if !meta.is_file()
            || std::num::NonZeroU32::new(
                meta.permissions().mode() & ((libc::S_IRWXG | libc::S_IRWXO) as u32),
            )
            .is_some()
            || meta.uid() != unsafe { libc::geteuid() }
        {
            return Err(format!(
                "token store {} is not a private file owned by this user",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    {
        if create {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!("create token store {}: {error}", path.display()))
                }
            }
        }
    }
    Ok(())
}

impl Store {
    pub fn open_existing() -> Result<Option<Self>, String> {
        let path = path();
        let dir = path.parent().expect("token store has parent");
        match fs::symlink_metadata(dir) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(format!(
                    "token directory {} is a symlink or not a directory",
                    dir.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "inspect token directory {}: {error}",
                    dir.display()
                ))
            }
        }
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                #[cfg(unix)]
                check_directory(path.parent().expect("token store has parent"), false)?;
                Self::open_at(&path, false).map(Some)
            }
            // No store, no secret to protect: the directory's mode matters
            // only once a store is created there, and `open` makes it private.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("inspect token store {}: {error}", path.display())),
        }
    }
    pub fn open() -> Result<Self, String> {
        let path = path();
        let dir = path.parent().expect("token store has parent");
        fs::create_dir_all(dir)
            .map_err(|error| format!("create token directory {}: {error}", dir.display()))?;
        #[cfg(unix)]
        check_directory(dir, true)?;
        Self::open_at(&path, true)
    }
    fn open_at(path: &Path, create: bool) -> Result<Self, String> {
        check_file(path, create)?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|error| format!("open token store {}: {error}", path.display()))?;
        if create {
            connection.execute_batch("CREATE TABLE IF NOT EXISTS tokens (
                    id TEXT PRIMARY KEY, label TEXT NOT NULL UNIQUE, endpoint TEXT NOT NULL,
                    secret TEXT NOT NULL, last_failure TEXT
                );
                CREATE TABLE IF NOT EXISTS selection (endpoint TEXT PRIMARY KEY, id TEXT NOT NULL);")
                .map_err(|error| format!("initialize token store {}: {error}", path.display()))?;
        }
        Ok(Self { connection })
    }
    pub fn list(&self) -> Result<Vec<TokenRow>, String> {
        let mut statement = self.connection.prepare("SELECT t.id AS id,t.label AS label,t.endpoint AS endpoint,
                t.last_failure AS last_failure, s.id IS NOT NULL AND s.id = t.id AS active FROM tokens t
                LEFT JOIN selection s ON s.endpoint = t.endpoint ORDER BY t.rowid")
            .map_err(|error| format!("prepare token list {}: {error}", path().display()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(TokenRow {
                    id: row.get("id")?,
                    label: row.get("label")?,
                    endpoint: row.get("endpoint")?,
                    last_failure: row.get("last_failure")?,
                    active: row.get("active")?,
                })
            })
            .map_err(|error| format!("read token list {}: {error}", path().display()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("decode token list {}: {error}", path().display()))
    }
    pub fn count_for(&self, endpoint: &str) -> Result<i64, String> {
        self.connection
            .query_row(
                "SELECT COUNT(*) AS count FROM tokens WHERE endpoint=?1",
                [endpoint],
                |row| row.get("count"),
            )
            .map_err(|error| format!("count stored model tokens for selected endpoint: {error}"))
    }
    pub fn add(label: &str, endpoint: &str, secret: &str) -> Result<TokenRow, String> {
        if label.trim().is_empty() || label != label.trim() || secret.trim().is_empty() {
            return Err("token add requires a non-empty label and token read from stdin".into());
        }
        let endpoint = canonical_endpoint(endpoint)?;
        Self::open()?.insert(label, endpoint, secret.trim())
    }
    fn insert(&mut self, label: &str, endpoint: String, secret: &str) -> Result<TokenRow, String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("begin token insertion for {label}: {error}"))?;
        let existing: Option<String> = tx
            .query_row("SELECT id FROM tokens WHERE label=?1", [label], |row| {
                row.get("id")
            })
            .optional()
            .map_err(|error| format!("inspect token label {label}: {error}"))?;
        if let Some(id) = existing {
            return Err(format!("token label {label} already exists as {id}; choose another label or remove that ID"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO tokens(id,label,endpoint,secret) VALUES (?1,?2,?3,?4)",
            params![&id, label, &endpoint, secret],
        )
        .map_err(|error| format!("store token {label}: {error}"))?;
        tx.commit()
            .map_err(|error| format!("commit token {label}: {error}"))?;
        Ok(TokenRow {
            id,
            label: label.into(),
            endpoint,
            active: false,
            last_failure: None,
        })
    }
    pub fn remove_existing(id: &str) -> Result<(), String> {
        match Self::open_existing()? {
            Some(mut store) => store.remove(id),
            None => Err(format!(
                "unknown token id: {id}; run `jeden token list` to see stored IDs"
            )),
        }
    }
    fn remove(&mut self, id: &str) -> Result<(), String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("begin token removal {id}: {error}"))?;
        let count = tx
            .execute("DELETE FROM tokens WHERE id=?1", [id])
            .map_err(|error| format!("remove token {id}: {error}"))?;
        if std::num::NonZeroUsize::new(count).is_none() {
            return Err(format!(
                "unknown token id: {id}; run `jeden token list` to see stored IDs"
            ));
        }
        tx.execute("DELETE FROM selection WHERE id=?1", [id])
            .map_err(|error| format!("forget token selection {id}: {error}"))?;
        tx.commit()
            .map_err(|error| format!("commit token removal {id}: {error}"))
    }
    pub fn next(
        &mut self,
        endpoint: &str,
        tried: &[String],
    ) -> Result<Option<SelectedToken>, String> {
        let endpoint = canonical_endpoint(endpoint)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("begin token selection for selected endpoint: {error}"))?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT id FROM selection WHERE endpoint=?1",
                params![&endpoint],
                |row| row.get("id"),
            )
            .optional()
            .map_err(|error| format!("read last selected token for selected endpoint: {error}"))?;
        let mut statement = tx
            .prepare("SELECT id FROM tokens WHERE endpoint=?1 ORDER BY rowid")
            .map_err(|error| format!("prepare token list for selected endpoint: {error}"))?;
        let ids = statement
            .query_map(params![&endpoint], |row| row.get::<_, String>("id"))
            .map_err(|error| format!("query token list for selected endpoint: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("decode token list for selected endpoint: {error}"))?;
        drop(statement);
        let mut ordered = ids.iter().cycle();
        if let Some(previous) = previous.filter(|id| ids.contains(id)) {
            for id in ordered.by_ref() {
                if id == &previous {
                    break;
                }
            }
        }
        let id = ordered.take(ids.len()).find(|id| !tried.contains(*id));
        let result = if let Some(id) = id {
            let secret = tx
                .query_row(
                    "SELECT secret FROM tokens WHERE id=?1",
                    params![id.as_str()],
                    |row| row.get("secret"),
                )
                .map_err(|error| format!("read selected token {id}: {error}"))?;
            tx.execute(
                "INSERT INTO selection(endpoint,id) VALUES (?1,?2)
                    ON CONFLICT(endpoint) DO UPDATE SET id=excluded.id",
                params![&endpoint, id.as_str()],
            )
            .map_err(|error| format!("record selected token {id}: {error}"))?;
            Some(SelectedToken {
                id: (*id).clone(),
                secret,
            })
        } else {
            None
        };
        tx.commit()
            .map_err(|error| format!("commit token selection for selected endpoint: {error}"))?;
        Ok(result)
    }
    pub fn failure(
        &self,
        id: &str,
        secret: &str,
        operation: &str,
        cause: &str,
    ) -> Result<(), String> {
        // A different process may remove this key while its request is in flight.
        // Redact with the selected key even when its row no longer exists.
        let safe_cause = if cause.contains(secret) {
            std::borrow::Cow::Owned(cause.replace(secret, "[REDACTED]"))
        } else {
            std::borrow::Cow::Borrowed(cause)
        };
        self.connection
            .execute(
                "UPDATE tokens SET last_failure=?2 WHERE id=?1",
                params![id, format!("{operation}: {safe_cause}")],
            )
            .map_err(|error| format!("record token failure: {error}"))?;
        Ok(())
    }
}
