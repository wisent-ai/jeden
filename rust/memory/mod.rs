use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCHEMA_VERSION: i64 = 1;
const MAX_MEMORY_CHARS: usize = 2_000;
const MAX_CONTEXT_CHARS: usize = 12_000;
const DEFAULT_LEASE_MS: i64 = 30_000;
const MAX_ATTEMPTS: i64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryScope { pub kind: String, pub id: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemorySource {
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryRecord {
    pub id: String,
    pub kind: String,
    pub scope: MemoryScope,
    pub text: String,
    pub tags: Vec<String>,
    pub source: MemorySource,
    pub confidence: f64,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecallHit {
    pub record: MemoryRecord,
    pub score: f64,
    pub provenance: RecallProvenance,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecallProvenance {
    pub backend: String,
    pub query: String,
    pub source: MemorySource,
}

pub(crate) trait SemanticBackend {
    fn name(&self) -> &'static str;
    fn recall(&self, conn: &Connection, scope: &MemoryScope, query: &str, limit: usize) -> Result<Vec<(String, f64)>, String>;
}

pub(crate) struct FtsBackend;
impl SemanticBackend for FtsBackend {
    fn name(&self) -> &'static str { "sqlite-fts5" }
    fn recall(&self, conn: &Connection, scope: &MemoryScope, query: &str, limit: usize) -> Result<Vec<(String, f64)>, String> {
        if query.trim().is_empty() {
            let mut stmt = conn.prepare("SELECT id, confidence FROM memories WHERE status='active' AND ((scope_kind=?1 AND scope_id=?2) OR scope_kind='global') ORDER BY updated_at DESC LIMIT ?3").map_err(|e| e.to_string())?;
            let rows = stmt.query_map(params![scope.kind, scope.id, limit as i64], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|e| e.to_string())?;
            let result = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
            return Ok(result);
        }
        let safe_query = query.split_whitespace().filter(|s| !s.is_empty()).map(|s| format!("\"{}\"*", s.replace('"', "\"\""))).collect::<Vec<_>>().join(" OR ");
        let mut stmt = conn.prepare("SELECT m.id, (-bm25(memories_fts) + m.confidence) AS score FROM memories_fts JOIN memories m ON m.rowid=memories_fts.rowid WHERE memories_fts MATCH ?1 AND m.status='active' AND ((m.scope_kind=?2 AND m.scope_id=?3) OR m.scope_kind='global') ORDER BY score DESC LIMIT ?4").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![safe_query, scope.kind, scope.id, limit as i64], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|e| e.to_string())?;
        let result = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        Ok(result)
    }
}

pub(crate) trait Consolidator {
    fn consolidate(&self, candidates: &[MemoryRecord], max_chars: usize) -> Result<String, String>;
}

pub(crate) struct MemoryStore { path: PathBuf }

impl MemoryStore {
    pub(crate) fn default_path() -> PathBuf {
        std::env::var_os("JEDEN_MEMORY_DB").map(PathBuf::from).or_else(|| std::env::var_os("JEDEN_MEMORY_FILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".jeden/memory.sqlite3"))
    }

    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let store = Self { path };
        let conn = store.connect()?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS memories(id TEXT PRIMARY KEY, kind TEXT NOT NULL, scope_kind TEXT NOT NULL, scope_id TEXT NOT NULL, text TEXT NOT NULL, tags_json TEXT NOT NULL, source_json TEXT NOT NULL, confidence REAL NOT NULL CHECK(confidence BETWEEN 0 AND 1), status TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS memories_scope ON memories(scope_kind, scope_id, status, updated_at DESC);
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(text, tags, kind, content='memories', content_rowid='rowid');
            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN INSERT INTO memories_fts(rowid,text,tags,kind) VALUES(new.rowid,new.text,new.tags_json,new.kind); END;
            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN INSERT INTO memories_fts(memories_fts,rowid,text,tags,kind) VALUES('delete',old.rowid,old.text,old.tags_json,old.kind); END;
            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN INSERT INTO memories_fts(memories_fts,rowid,text,tags,kind) VALUES('delete',old.rowid,old.text,old.tags_json,old.kind); INSERT INTO memories_fts(rowid,text,tags,kind) VALUES(new.rowid,new.text,new.tags_json,new.kind); END;
            CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload_json TEXT NOT NULL, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, available_at INTEGER NOT NULL, lease_owner TEXT, lease_until INTEGER, heartbeat_at INTEGER, last_error TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS jobs_claim ON jobs(state, available_at, lease_until);
            CREATE TABLE IF NOT EXISTS scope_locks(scope_kind TEXT NOT NULL, scope_id TEXT NOT NULL, owner TEXT NOT NULL, expires_at INTEGER NOT NULL, PRIMARY KEY(scope_kind,scope_id));
            CREATE TABLE IF NOT EXISTS workflows(fingerprint TEXT PRIMARY KEY, description TEXT NOT NULL, sessions_json TEXT NOT NULL, occurrences INTEGER NOT NULL, verified INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS managed_skills(id TEXT PRIMARY KEY, workflow_fingerprint TEXT NOT NULL UNIQUE REFERENCES workflows(fingerprint), body TEXT NOT NULL, created_at INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO metadata(key,value) VALUES('schema_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [SCHEMA_VERSION.to_string()]).map_err(|e| e.to_string())?;
        Ok(store)
    }

    fn connect(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(10)).map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA foreign_keys=ON;").map_err(|e| e.to_string())?;
        Ok(conn)
    }

    pub(crate) fn path(&self) -> &Path { &self.path }

    pub(crate) fn remember(&self, kind: &str, scope: &MemoryScope, text: &str, tags: &[String], source: &MemorySource, confidence: f64) -> Result<MemoryRecord, String> {
        let text = bounded_redacted(text, MAX_MEMORY_CHARS);
        if text.is_empty() { return Err("memory text is empty after redaction".into()); }
        let now = now_ms();
        let id = stable_id("mem");
        let record = MemoryRecord { id: id.clone(), kind: kind.to_string(), scope: scope.clone(), text, tags: tags.to_vec(), source: source.clone(), confidence: confidence.clamp(0.0,1.0), status: "active".into(), created_at: now, updated_at: now };
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO memories(id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![record.id,record.kind,record.scope.kind,record.scope.id,record.text,serde_json::to_string(&record.tags).map_err(|e|e.to_string())?,serde_json::to_string(&record.source).map_err(|e|e.to_string())?,record.confidence,record.status,now,now]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(record)
    }

    pub(crate) fn list(&self, limit: usize) -> Result<Vec<MemoryRecord>, String> {
        let conn=self.connect()?; let mut stmt=conn.prepare("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at FROM memories ORDER BY updated_at DESC LIMIT ?1").map_err(|e|e.to_string())?;
        let rows=stmt.query_map([limit.min(500) as i64], row_record).map_err(|e|e.to_string())?;
        let result=rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
        Ok(result)
    }

    pub(crate) fn recall(&self, backend: &dyn SemanticBackend, scope: &MemoryScope, query: &str, limit: usize) -> Result<Vec<RecallHit>, String> {
        let conn=self.connect()?; let ranked=backend.recall(&conn,scope,query,limit.min(100))?; let mut hits=Vec::new();
        for (id,score) in ranked { if let Some(record)=load_record(&conn,&id)? { hits.push(RecallHit { provenance: RecallProvenance { backend: backend.name().into(), query: query.into(), source: record.source.clone() }, record, score }); } }
        Ok(hits)
    }

    pub(crate) fn forget_scope(&self, scope: &MemoryScope) -> Result<usize,String> { let conn=self.connect()?; conn.execute("UPDATE memories SET status='forgotten',updated_at=?3 WHERE scope_kind=?1 AND scope_id=?2 AND status='active'",params![scope.kind,scope.id,now_ms()]).map_err(|e|e.to_string()) }
    pub(crate) fn clear(&self) -> Result<usize,String> { let conn=self.connect()?; conn.execute("DELETE FROM memories",[]).map_err(|e|e.to_string()) }

    pub(crate) fn enqueue(&self, kind:&str, payload:&Value) -> Result<String,String> { let conn=self.connect()?; let id=stable_id("job"); let now=now_ms(); conn.execute("INSERT INTO jobs(id,kind,payload_json,state,available_at,created_at,updated_at) VALUES(?1,?2,?3,'queued',?4,?4,?4)",params![id,kind,serde_json::to_string(payload).map_err(|e|e.to_string())?,now]).map_err(|e|e.to_string())?; Ok(id) }

    pub(crate) fn claim(&self, worker:&str, lease_ms:Option<i64>) -> Result<Option<LeasedJob>,String> { let now=now_ms(); let until=now+lease_ms.unwrap_or(DEFAULT_LEASE_MS).clamp(1_000,300_000); let mut conn=self.connect()?; let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?; let id:Option<String>=tx.query_row("SELECT id FROM jobs WHERE attempts < ?1 AND available_at<=?2 AND (state='queued' OR (state='leased' AND lease_until<?2)) ORDER BY created_at LIMIT 1",params![MAX_ATTEMPTS,now],|r|r.get(0)).optional().map_err(|e|e.to_string())?; let Some(id)=id else { tx.commit().map_err(|e|e.to_string())?; return Ok(None) }; tx.execute("UPDATE jobs SET state='leased',lease_owner=?2,lease_until=?3,heartbeat_at=?4,attempts=attempts+1,updated_at=?4 WHERE id=?1",params![id,worker,until,now]).map_err(|e|e.to_string())?; let job=tx.query_row("SELECT id,kind,payload_json,attempts,lease_until FROM jobs WHERE id=?1",[&id],|r|Ok(LeasedJob{id:r.get(0)?,kind:r.get(1)?,payload:serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap_or(Value::Null),attempts:r.get(3)?,lease_until:r.get(4)?})).map_err(|e|e.to_string())?; tx.commit().map_err(|e|e.to_string())?; Ok(Some(job)) }
    pub(crate) fn heartbeat(&self,id:&str,worker:&str,lease_ms:i64)->Result<bool,String>{let now=now_ms();let conn=self.connect()?;Ok(conn.execute("UPDATE jobs SET heartbeat_at=?3,lease_until=?4,updated_at=?3 WHERE id=?1 AND state='leased' AND lease_owner=?2",params![id,worker,now,now+lease_ms.clamp(1_000,300_000)]).map_err(|e|e.to_string())?==1)}
    pub(crate) fn complete(&self,id:&str,worker:&str)->Result<bool,String>{let conn=self.connect()?;Ok(conn.execute("UPDATE jobs SET state='done',lease_owner=NULL,lease_until=NULL,updated_at=?3 WHERE id=?1 AND state='leased' AND lease_owner=?2",params![id,worker,now_ms()]).map_err(|e|e.to_string())?==1)}
    pub(crate) fn retry(&self,id:&str,worker:&str,error:&str)->Result<bool,String>{let conn=self.connect()?; let now=now_ms(); let attempts:i64=conn.query_row("SELECT attempts FROM jobs WHERE id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?; let state=if attempts>=MAX_ATTEMPTS{"failed"}else{"queued"}; let delay=(1_i64<<attempts.min(8))*1_000; Ok(conn.execute("UPDATE jobs SET state=?3,available_at=?4,lease_owner=NULL,lease_until=NULL,last_error=?5,updated_at=?6 WHERE id=?1 AND lease_owner=?2",params![id,worker,state,now+delay,bounded_redacted(error,500),now]).map_err(|e|e.to_string())?==1)}

    pub(crate) fn process_one(&self, worker: &str) -> Result<bool, String> {
        let Some(job) = self.claim(worker, None)? else { return Ok(false); };
        if !self.heartbeat(&job.id, worker, DEFAULT_LEASE_MS)? { return Err("memory job lease was lost before processing".into()); }
        let outcome = match job.kind.as_str() {
            "extract" => {
                let scope: MemoryScope = serde_json::from_value(job.payload.get("scope").cloned().ok_or("extract job missing scope")?).map_err(|e| e.to_string())?;
                let text = job.payload.get("text").and_then(Value::as_str).ok_or("extract job missing text")?;
                let source = MemorySource { origin: "session_extraction".into(), session_id: job.payload.get("sessionId").and_then(Value::as_str).map(str::to_string), entry_id: job.payload.get("entryId").and_then(Value::as_str).map(str::to_string) };
                self.remember("session", &scope, text, &["automatic".into()], &source, 0.6).map(|_| ())
            }
            other => Err(format!("unsupported memory job kind: {other}")),
        };
        match outcome {
            Ok(()) => { if !self.complete(&job.id, worker)? { return Err("memory job lease was lost before completion".into()); } Ok(true) }
            Err(error) => { self.retry(&job.id, worker, &error)?; Err(error) }
        }
    }

    pub(crate) fn acquire_scope_lock(&self,scope:&MemoryScope,owner:&str,ttl_ms:i64)->Result<bool,String>{let now=now_ms();let mut conn=self.connect()?;let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;tx.execute("DELETE FROM scope_locks WHERE expires_at<?1",[now]).map_err(|e|e.to_string())?;let acquired=tx.execute("INSERT INTO scope_locks(scope_kind,scope_id,owner,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(scope_kind,scope_id) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at WHERE scope_locks.owner=excluded.owner",params![scope.kind,scope.id,owner,now+ttl_ms.clamp(1_000,300_000)]).map_err(|e|e.to_string())?==1;tx.commit().map_err(|e|e.to_string())?;Ok(acquired)}
    pub(crate) fn release_scope_lock(&self,scope:&MemoryScope,owner:&str)->Result<(),String>{self.connect()?.execute("DELETE FROM scope_locks WHERE scope_kind=?1 AND scope_id=?2 AND owner=?3",params![scope.kind,scope.id,owner]).map_err(|e|e.to_string())?;Ok(())}

    pub(crate) fn pre_compaction_context(&self,scope:&MemoryScope,query:&str,max_chars:usize)->Result<String,String>{let hits=self.recall(&FtsBackend,scope,query,100)?;let cap=max_chars.min(MAX_CONTEXT_CHARS);let mut out=String::new();for hit in hits{let line=format!("[{}; {}; {}] {}\n",hit.record.id,hit.provenance.backend,hit.record.source.origin,hit.record.text);if out.chars().count()+line.chars().count()>cap{break}out.push_str(&line)}Ok(out)}
    pub(crate) fn consolidate(&self,scope:&MemoryScope,model:&dyn Consolidator,max_chars:usize)->Result<MemoryRecord,String>{let candidates=self.recall(&FtsBackend,scope,"",100)?.into_iter().map(|h|h.record).collect::<Vec<_>>();if candidates.len()<2{return Err("consolidation requires at least two memories".into())}let text=bounded_redacted(&model.consolidate(&candidates,max_chars.min(MAX_MEMORY_CHARS))?,max_chars.min(MAX_MEMORY_CHARS));self.remember("summary",scope,&text,&["consolidated".into()],&MemorySource{origin:"model_consolidation".into(),session_id:None,entry_id:None},0.7)}
    pub(crate) fn persist_model_consolidation(&self, scope: &MemoryScope, summary: &str) -> Result<MemoryRecord, String> {
        self.remember("summary", scope, summary, &["consolidated".into(), "model-assisted".into()], &MemorySource { origin: "model_compaction".into(), session_id: None, entry_id: None }, 0.85)
    }

    pub(crate) fn record_workflow(&self,fingerprint:&str,description:&str,session_id:&str,verified:bool)->Result<Option<String>,String>{let mut conn=self.connect()?;let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;let prior:Option<(String,i64,i64)>=tx.query_row("SELECT sessions_json,occurrences,verified FROM workflows WHERE fingerprint=?1",[fingerprint],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;let(mut sessions,occ,was_verified)=prior.map(|(j,o,v)|(serde_json::from_str::<Vec<String>>(&j).unwrap_or_default(),o,v)).unwrap_or((Vec::new(),0,0));if !sessions.iter().any(|s|s==session_id){sessions.push(session_id.into())}let occurrences=occ+1;let is_verified=was_verified==1||verified;tx.execute("INSERT INTO workflows(fingerprint,description,sessions_json,occurrences,verified,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(fingerprint) DO UPDATE SET description=excluded.description,sessions_json=excluded.sessions_json,occurrences=excluded.occurrences,verified=excluded.verified,updated_at=excluded.updated_at",params![fingerprint,bounded_redacted(description,1000),serde_json::to_string(&sessions).map_err(|e|e.to_string())?,occurrences,is_verified as i64,now_ms()]).map_err(|e|e.to_string())?;let skill=if is_verified&&sessions.len()>=3&&occurrences>=3{let id=format!("skill-{}",fingerprint);let body=format!("Verified workflow ({} independent sessions): {}",sessions.len(),bounded_redacted(description,1000));tx.execute("INSERT INTO managed_skills(id,workflow_fingerprint,body,created_at) VALUES(?1,?2,?3,?4) ON CONFLICT(workflow_fingerprint) DO UPDATE SET body=excluded.body",params![id,fingerprint,body,now_ms()]).map_err(|e|e.to_string())?;Some(id)}else{None};tx.commit().map_err(|e|e.to_string())?;Ok(skill)}

    pub(crate) fn health(&self)->Result<Value,String>{let conn=self.connect()?;let memories:i64=conn.query_row("SELECT count(*) FROM memories WHERE status='active'",[],|r|r.get(0)).map_err(|e|e.to_string())?;let queued:i64=conn.query_row("SELECT count(*) FROM jobs WHERE state IN ('queued','leased')",[],|r|r.get(0)).map_err(|e|e.to_string())?;Ok(json!({"service":"memory","healthy":true,"backend":"sqlite-wal-fts5","schemaVersion":SCHEMA_VERSION,"path":self.path,"activeMemories":memories,"pendingJobs":queued,"bounded":{"memoryChars":MAX_MEMORY_CHARS,"contextChars":MAX_CONTEXT_CHARS,"attempts":MAX_ATTEMPTS}}))}
}

#[derive(Debug,Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) struct LeasedJob { pub id:String,pub kind:String,pub payload:Value,pub attempts:i64,pub lease_until:i64 }

pub(crate) fn scope_from_value(value:Option<&Value>,cwd:&Path)->MemoryScope{match value{Some(Value::Object(m))=>{let kind=m.get("kind").and_then(Value::as_str).unwrap_or("repo").to_string();let id=m.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(||if kind=="repo"{cwd.display().to_string()}else{kind.clone()});MemoryScope{kind,id}},Some(Value::String(s)) if s!="repo"=>MemoryScope{kind:s.clone(),id:s.clone()},_=>MemoryScope{kind:"repo".into(),id:cwd.display().to_string()}}}

pub(crate) fn extract_ledger_entry(store:&MemoryStore,session_id:&str,entry:&crate::cli::sessions::LedgerEntry,scope:&MemoryScope)->Result<Option<String>,String>{let text=match entry.kind.as_str(){"user"|"assistant"|"message"=>entry.data.get("content").or_else(||entry.data.get("text")).and_then(Value::as_str),"tool_result"=>entry.data.get("replayMessage").and_then(Value::as_str),_=>None};let Some(text)=text else{return Ok(None)};if text.trim().len()<24{return Ok(None)};store.enqueue("extract",&json!({"sessionId":session_id,"entryId":entry.id,"scope":scope,"text":bounded_redacted(text,MAX_CONTEXT_CHARS)})).map(Some)}

fn load_record(conn:&Connection,id:&str)->Result<Option<MemoryRecord>,String>{conn.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at FROM memories WHERE id=?1",[id],row_record).optional().map_err(|e|e.to_string())}
fn row_record(row:&rusqlite::Row<'_>)->rusqlite::Result<MemoryRecord>{let tags_json:String=row.get(5)?;let source_json:String=row.get(6)?;Ok(MemoryRecord{id:row.get(0)?,kind:row.get(1)?,scope:MemoryScope{kind:row.get(2)?,id:row.get(3)?},text:row.get(4)?,tags:serde_json::from_str(&tags_json).unwrap_or_default(),source:serde_json::from_str(&source_json).unwrap_or(MemorySource{origin:"unknown".into(),session_id:None,entry_id:None}),confidence:row.get(7)?,status:row.get(8)?,created_at:row.get(9)?,updated_at:row.get(10)?})}
fn stable_id(prefix:&str)->String{use rand::RngCore;let mut bytes=[0u8;16];rand::thread_rng().fill_bytes(&mut bytes);format!("{prefix}_{}",hex::encode(bytes))}
fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis().min(i64::MAX as u128) as i64}
pub(crate) fn bounded_redacted(value:&str,max:usize)->String{let mut text=value.to_string();for pattern in [r"\b(?:sk|pk|rk)_[A-Za-z0-9_\-]{12,}\b",r"\bgh[pousr]_[A-Za-z0-9_]{16,}\b",r"(?i)\b(?:password|passwd|token|secret|api[_-]?key)\s*[:=]\s*[^\s,;]+",r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----"]{if let Ok(re)=regex::Regex::new(pattern){text=re.replace_all(&text,"[REDACTED]").into_owned()}}let normalized=text.split_whitespace().collect::<Vec<_>>().join(" ");if normalized.chars().count()>max{let mut out=normalized.chars().take(max.saturating_sub(1)).collect::<String>();out.push('…');out}else{normalized}}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        dir: PathBuf,
        db: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "jeden-memory-runtime-{name}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let db = dir.join("memory.sqlite3");
            Self { dir, db }
        }

        fn store(&self) -> MemoryStore {
            MemoryStore::open(&self.db).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn scope() -> MemoryScope {
        MemoryScope { kind: "repo".into(), id: "runtime-tests".into() }
    }

    fn source() -> MemorySource {
        MemorySource { origin: "test".into(), session_id: None, entry_id: None }
    }

    #[test]
    fn memory_runtime_concurrent_transactional_writes_are_all_durable_with_unique_ids() {
        const WRITERS: usize = 16;
        let fixture = Fixture::new("concurrent-writes");
        fixture.store();
        let barrier = Arc::new(Barrier::new(WRITERS));
        let mut writers = Vec::new();

        for writer in 0..WRITERS {
            let db = fixture.db.clone();
            let barrier = barrier.clone();
            writers.push(std::thread::spawn(move || {
                let store = MemoryStore::open(db).unwrap();
                barrier.wait();
                store
                    .remember(
                        "fact",
                        &scope(),
                        &format!("transactional memory {writer}"),
                        &[],
                        &source(),
                        0.8,
                    )
                    .unwrap()
                    .id
            }));
        }

        let ids = writers.into_iter().map(|writer| writer.join().unwrap()).collect::<HashSet<_>>();
        let records = fixture.store().list(WRITERS).unwrap();
        assert_eq!(ids.len(), WRITERS, "a duplicate generated ID lost a concurrent write");
        assert_eq!(records.len(), WRITERS);
        assert_eq!(records.iter().map(|record| record.id.as_str()).collect::<HashSet<_>>().len(), WRITERS);
        assert!(records.iter().all(|record| record.id.starts_with("mem_") && record.id.len() == 36));
    }

    #[test]
    fn memory_runtime_expired_lease_is_reclaimed_while_heartbeat_and_retry_remain_owner_safe() {
        let fixture = Fixture::new("lease-reclaim");
        let store = fixture.store();
        let id = store.enqueue("extract", &json!({"text": "durable payload"})).unwrap();
        let first = store.claim("worker-a", Some(60_000)).unwrap().unwrap();
        assert_eq!(first.id, id);
        assert_eq!(first.attempts, 1);
        assert!(store.heartbeat(&id, "worker-a", 120_000).unwrap());
        let extended_until: i64 = store.connect().unwrap().query_row("SELECT lease_until FROM jobs WHERE id=?1", [&id], |row| row.get(0)).unwrap();
        assert!(extended_until > first.lease_until);
        assert!(!store.heartbeat(&id, "worker-b", 60_000).unwrap());

        store.connect().unwrap().execute("UPDATE jobs SET lease_until=0 WHERE id=?1", [&id]).unwrap();
        let reclaimed = store.claim("worker-b", Some(60_000)).unwrap().unwrap();
        assert_eq!(reclaimed.id, id);
        assert_eq!(reclaimed.attempts, 2);
        assert!(!store.retry(&id, "worker-a", "wrong owner").unwrap());
        assert!(store.retry(&id, "worker-b", "token=super-secret-value").unwrap());

        let conn = store.connect().unwrap();
        let (state, owner, error): (String, Option<String>, String) = conn
            .query_row(
                "SELECT state,lease_owner,last_error FROM jobs WHERE id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "queued");
        assert_eq!(owner, None);
        assert_eq!(error, "[REDACTED]");
        drop(conn);

        for expected_attempt in 3..=MAX_ATTEMPTS {
            store.connect().unwrap().execute("UPDATE jobs SET available_at=0 WHERE id=?1", [&id]).unwrap();
            let lease = store.claim("worker-b", None).unwrap().unwrap();
            assert_eq!(lease.attempts, expected_attempt);
            assert!(store.retry(&id, "worker-b", "retry failure").unwrap());
        }
        let final_state: String = store.connect().unwrap().query_row("SELECT state FROM jobs WHERE id=?1", [&id], |row| row.get(0)).unwrap();
        assert_eq!(final_state, "failed");
        assert!(store.claim("worker-c", None).unwrap().is_none());
    }

    #[test]
    fn memory_runtime_recorder_extraction_redacts_secrets_before_the_job_is_persisted() {
        let fixture = Fixture::new("recorder-redaction");
        let store = fixture.store();
        let entry = crate::cli::sessions::LedgerEntry {
            version: crate::cli::sessions::SESSION_LEDGER_VERSION,
            id: "entry-1".into(),
            parent_id: None,
            ts: "1".into(),
            kind: "user".into(),
            data: json!({"content": "remember this durable fact token=super-secret-value for later"}),
        };

        let job_id = extract_ledger_entry(&store, "session-1", &entry, &scope()).unwrap().unwrap();
        let payload: String = store.connect().unwrap().query_row("SELECT payload_json FROM jobs WHERE id=?1", [&job_id], |row| row.get(0)).unwrap();
        assert!(!payload.contains("super-secret-value"));
        assert!(payload.contains("[REDACTED]"));
        assert!(payload.contains("session-1"));

        let short = crate::cli::sessions::LedgerEntry { data: json!({"content": "too short"}), ..entry };
        assert_eq!(extract_ledger_entry(&store, "session-1", &short, &scope()).unwrap(), None);
    }

    struct RecordingConsolidator {
        candidate_count: Cell<usize>,
        requested_limit: Cell<usize>,
    }

    impl Consolidator for RecordingConsolidator {
        fn consolidate(&self, candidates: &[MemoryRecord], max_chars: usize) -> Result<String, String> {
            self.candidate_count.set(candidates.len());
            self.requested_limit.set(max_chars);
            Ok(format!("password=super-secret-value {}", "summary ".repeat(40)))
        }
    }

    #[test]
    fn memory_runtime_precompaction_and_model_consolidation_obey_bounds_without_partial_records_or_secrets() {
        let fixture = Fixture::new("bounded-consolidation");
        let store = fixture.store();
        for text in ["first durable memory", "second durable memory"] {
            store.remember("fact", &scope(), text, &[], &source(), 0.8).unwrap();
        }

        let complete = store.pre_compaction_context(&scope(), "", MAX_CONTEXT_CHARS).unwrap();
        let first_line = complete.split_inclusive('\n').next().unwrap();
        let bounded = store.pre_compaction_context(&scope(), "", first_line.chars().count()).unwrap();
        assert_eq!(bounded, first_line);
        let below_record_boundary = store.pre_compaction_context(&scope(), "", first_line.chars().count() - 1).unwrap();
        assert!(below_record_boundary.is_empty(), "precompaction must not emit a truncated record");

        let model = RecordingConsolidator { candidate_count: Cell::new(0), requested_limit: Cell::new(0) };
        let summary = store.consolidate(&scope(), &model, 96).unwrap();
        assert_eq!(model.candidate_count.get(), 2);
        assert_eq!(model.requested_limit.get(), 96);
        assert!(summary.text.chars().count() <= 96);
        assert!(!summary.text.contains("super-secret-value"));
        assert!(summary.text.contains("[REDACTED]"));
        assert_eq!(summary.kind, "summary");
        assert_eq!(summary.source.origin, "model_consolidation");
    }
}
