use super::{MAX_BLOB_BYTES, MAX_ROOM_EVENTS};
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
const COLLAB_SCHEMA_VERSION: u32 = 2;
fn collab_migration_marker(value: &mut serde_json::Value) -> Result<(), String> {
    crate::cli::config::migrations::object_preflight(value)
}
static COLLAB_MIGRATION_STEPS: [crate::cli::config::migrations::MigrationStep; 2] = [
    crate::cli::config::migrations::MigrationStep {
        name: "legacy-schema-baseline",
        from: 0,
        to: 1,
        apply: collab_migration_marker,
    },
    crate::cli::config::migrations::MigrationStep {
        name: "relay-migration-history",
        from: 1,
        to: 2,
        apply: collab_migration_marker,
    },
];
fn collab_migration_plan() -> crate::cli::config::migrations::MigrationPlan {
    crate::cli::config::migrations::MigrationPlan {
        store: "collab",
        from: 0,
        to: COLLAB_SCHEMA_VERSION,
        reversible: true,
        preflight: crate::cli::config::migrations::object_preflight,
        steps: &COLLAB_MIGRATION_STEPS,
        compatibility_window: crate::cli::config::migrations::CompatibilityWindow {
            oldest_readable: 0,
            newest_readable: 2,
            rollback_floor: 1,
        },
    }
}
fn migrate_collab(path: &Path) -> Result<crate::cli::config::migrations::MigrationOutcome, String> {
    crate::cli::config::migrations::migrate_sqlite(path, &collab_migration_plan(), |tx, _, to| {
        if to == 2 {
            tx.execute_batch("CREATE TABLE IF NOT EXISTS migration_history(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); INSERT OR IGNORE INTO migration_history(version,applied_at) VALUES(2,unixepoch('now'));").map_err(|e|e.to_string())?;
        }
        Ok(())
    })
}

pub struct RelayStore {
    path: PathBuf,
}
impl RelayStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?
        }
        let store = Self { path };
        let conn = store.connect()?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS rooms(id TEXT PRIMARY KEY, created_at INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS room_tokens(room_id TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE, role TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE, generation INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(room_id,role));
          CREATE TABLE IF NOT EXISTS events(room_id TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE, seq INTEGER NOT NULL, blob TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(room_id,seq));
          CREATE INDEX IF NOT EXISTS relay_events_room ON events(room_id,seq);").map_err(|e|e.to_string())?;
        drop(conn);
        migrate_collab(&store.path)?;
        Ok(store)
    }
    pub fn new() -> Self {
        let path = std::env::var_os("JEDEN_COLLAB_RELAY_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
                    .join(".jeden/collab-relay.sqlite3")
            });
        Self::open(path).expect("open collab relay store")
    }
    fn connect(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(10))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| e.to_string())?;
        Ok(conn)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn post_authorized(
        &self,
        room: &str,
        blob: String,
        token: Option<&str>,
    ) -> Result<Option<usize>, String> {
        let token = token.ok_or("write token required")?;
        let role = token_role(token).ok_or("invalid role-bound token")?;
        if role == "view" {
            return Err("view role is read-only".into());
        }
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM rooms WHERE id=?1)",
                [room],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            if role != "full" {
                return Err("full token required to create room".into());
            }
            tx.execute(
                "INSERT INTO rooms(id,created_at) VALUES(?1,?2)",
                params![room, now_ms()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO room_tokens(room_id,role,token_hash) VALUES(?1,'full',?2)",
                params![room, token_hash(token)],
            )
            .map_err(|e| e.to_string())?;
        }
        let authorized:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM room_tokens WHERE room_id=?1 AND role=?2 AND token_hash=?3)",params![room,role,token_hash(token)],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !authorized {
            return Err("unauthorized room write".into());
        }
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM events WHERE room_id=?1",
                [room],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count >= MAX_ROOM_EVENTS as i64 {
            return Ok(None);
        }
        let seq = count + 1;
        tx.execute(
            "INSERT INTO events(room_id,seq,blob,created_at) VALUES(?1,?2,?3,?4)",
            params![room, seq, blob, now_ms()],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(seq as usize))
    }
    pub fn get(&self, room: &str, since: usize) -> (Vec<String>, usize) {
        self.get_result(room, since).unwrap_or_default()
    }
    fn get_result(&self, room: &str, since: usize) -> Result<(Vec<String>, usize), String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT blob FROM events WHERE room_id=?1 AND seq>?2 ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room, since as i64], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let events = rows
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| e.to_string())?;
        let next: i64 = conn
            .query_row(
                "SELECT coalesce(max(seq),0) FROM events WHERE room_id=?1",
                [room],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok((events, next as usize))
    }
    pub fn rotate_token(&self, room: &str, old: &str, new: &str) -> Result<bool, String> {
        let old_role = token_role(old).ok_or("invalid old role token")?;
        let new_role = token_role(new).ok_or("invalid new role token")?;
        if old_role == "view" || new_role == "view" {
            return Ok(false);
        }
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let authorized:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM room_tokens WHERE room_id=?1 AND role=?2 AND token_hash=?3)",params![room,old_role,token_hash(old)],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !authorized {
            return Ok(false);
        }
        if old_role == "full" && new_role != "full" {
            tx.execute("INSERT INTO room_tokens(room_id,role,token_hash) VALUES(?1,?2,?3) ON CONFLICT(room_id,role) DO UPDATE SET token_hash=excluded.token_hash,generation=room_tokens.generation+1",params![room,new_role,token_hash(new)]).map_err(|e|e.to_string())?;
        } else if old_role == new_role {
            tx.execute("UPDATE room_tokens SET token_hash=?3,generation=generation+1 WHERE room_id=?1 AND role=?2",params![room,old_role,token_hash(new)]).map_err(|e|e.to_string())?;
        } else {
            return Err("only full tokens may provision another role".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
    pub fn health(&self) -> Result<serde_json::Value, String> {
        let conn = self.connect()?;
        let rooms: i64 = conn
            .query_row("SELECT count(*) FROM rooms", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let events: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let tokens: i64 = conn
            .query_row("SELECT count(*) FROM room_tokens", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        Ok(
            json!({"ok":true,"service":"jeden-collab-relay","backend":"sqlite-wal","schemaVersion":COLLAB_SCHEMA_VERSION,"contentBlind":true,"rooms":rooms,"events":events,"roleTokens":tokens,"path":self.path}),
        )
    }
}

fn relay_response_authorized(
    store: &RelayStore,
    method: &str,
    path: &str,
    query: &str,
    body: &str,
    token: Option<&str>,
) -> (u16, String) {
    if method == "GET" && path == "/health" {
        return match store.health() {
            Ok(v) => (200, v.to_string()),
            Err(e) => (500, json!({"ok":false,"error":e}).to_string()),
        };
    }
    let target = match path.strip_prefix("/room/") {
        Some(r) if !r.is_empty() => r,
        _ => return (404, json!({"ok":false,"error":"not found"}).to_string()),
    };
    if method == "PUT" {
        if let Some(room) = target.strip_suffix("/token") {
            return match token {
                Some(old) => match store.rotate_token(room, old, body.trim()) {
                    Ok(true) => (200, json!({"ok":true}).to_string()),
                    Ok(false) => (403, json!({"ok":false,"error":"unauthorized"}).to_string()),
                    Err(e) => (400, json!({"ok":false,"error":e}).to_string()),
                },
                None => (
                    403,
                    json!({"ok":false,"error":"write token required"}).to_string(),
                ),
            };
        }
    }
    match method {
        "POST" => {
            let blob = body.trim();
            if blob.is_empty() {
                return (400, json!({"ok":false,"error":"empty body"}).to_string());
            }
            if blob.len() > MAX_BLOB_BYTES {
                return (
                    413,
                    json!({"ok":false,"error":"payload too large"}).to_string(),
                );
            }
            match store.post_authorized(target, blob.to_string(), token) {
                Ok(Some(seq)) => (200, json!({"ok":true,"seq":seq}).to_string()),
                Ok(None) => (429, json!({"ok":false,"error":"room is full"}).to_string()),
                Err(e) => (403, json!({"ok":false,"error":e}).to_string()),
            }
        }
        "GET" => {
            let (events, next) = store.get(target, parse_since(query));
            (
                200,
                json!({"ok":true,"events":events,"next":next}).to_string(),
            )
        }
        _ => (
            405,
            json!({"ok":false,"error":"method not allowed"}).to_string(),
        ),
    }
}
fn parse_since(query: &str) -> usize {
    query
        .split('&')
        .find_map(|p| p.strip_prefix("since="))
        .and_then(|v| v.parse().ok())
        .unwrap_or_default()
}
pub fn serve(addr: &str) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    let store = std::sync::Arc::new(RelayStore::new());
    println!(
        "jeden collab-relay listening on http://{bound} (durable {})",
        store.path().display()
    );
    for stream in listener.incoming().flatten() {
        let store = store.clone();
        std::thread::spawn(move || {
            let _ = handle_conn(stream, &store);
        });
    }
    Ok(())
}
fn handle_conn(mut stream: TcpStream, store: &RelayStore) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            break buf.len();
        }
    };
    let header = String::from_utf8_lossy(&buf[..header_end.min(buf.len())]).to_string();
    let (method, path, query) = parse_request_line(&header);
    let length = parse_content_length(&header);
    if length > MAX_BLOB_BYTES {
        write_response(
            &mut stream,
            413,
            &json!({"ok":false,"error":"payload too large"}).to_string(),
        )?;
        return Ok(());
    }
    while buf.len() < header_end + length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n])
    }
    let body = String::from_utf8_lossy(&buf[header_end.min(buf.len())..]).to_string();
    let token = header_value(&header, "x-jeden-write-token");
    let requested_role = header_value(&header, "x-jeden-role");
    let (status, response) = match (token.as_deref(), requested_role.as_deref()) {
        (Some(token), Some(role)) if token_role(token) != Some(role) => (
            403,
            json!({"ok":false,"error":"write token is not valid for requested role"}).to_string(),
        ),
        _ => relay_response_authorized(store, &method, &path, &query, &body, token.as_deref()),
    };
    write_response(&mut stream, status, &response)
}
fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let response=format!("HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type,x-jeden-write-token,x-jeden-role\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",body.len(),body);
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
fn find_subsequence(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}
pub(super) fn parse_request_line(header: &str) -> (String, String, String) {
    let mut p = header.lines().next().unwrap_or("").split_whitespace();
    let method = p.next().unwrap_or("").to_string();
    let target = p.next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    (method, path.to_string(), query.to_string())
}
pub(super) fn parse_content_length(header: &str) -> usize {
    header_value(header, "content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or_default()
}
fn header_value(header: &str, name: &str) -> Option<String> {
    header.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}
fn token_role(token: &str) -> Option<&str> {
    let (role, _) = token.split_once('.')?;
    matches!(role, "view" | "prompt" | "abort" | "full").then_some(role)
}
fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
