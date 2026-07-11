use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::json;
use sha2::{Digest,Sha256};
use std::io::{Read,Write};
use std::net::{TcpListener,TcpStream};
use std::path::{Path,PathBuf};
use std::time::Duration;
use super::{MAX_BLOB_BYTES,MAX_ROOM_EVENTS};

pub struct RelayStore { path:PathBuf }
impl RelayStore {
    pub fn open(path:impl AsRef<Path>)->Result<Self,String>{
        let path=path.as_ref().to_path_buf();if let Some(parent)=path.parent(){std::fs::create_dir_all(parent).map_err(|e|e.to_string())?}
        let store=Self{path};let conn=store.connect()?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS rooms(id TEXT PRIMARY KEY, created_at INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS room_tokens(room_id TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE, role TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE, generation INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(room_id,role));
          CREATE TABLE IF NOT EXISTS events(room_id TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE, seq INTEGER NOT NULL, blob TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(room_id,seq));
          CREATE INDEX IF NOT EXISTS relay_events_room ON events(room_id,seq);").map_err(|e|e.to_string())?;Ok(store)
    }
    pub fn new()->Self{let path=std::env::var_os("JEDEN_COLLAB_RELAY_DB").map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("HOME").unwrap_or_else(||".".into())).join(".jeden/collab-relay.sqlite3"));Self::open(path).expect("open collab relay store")}
    fn connect(&self)->Result<Connection,String>{let conn=Connection::open(&self.path).map_err(|e|e.to_string())?;conn.busy_timeout(Duration::from_secs(10)).map_err(|e|e.to_string())?;conn.execute_batch("PRAGMA foreign_keys=ON;").map_err(|e|e.to_string())?;Ok(conn)}
    pub fn path(&self)->&Path{&self.path}
    pub fn post(&self,room:&str,blob:String)->Option<usize>{self.post_authorized(room,blob,None).ok().flatten()}
    pub fn post_authorized(&self,room:&str,blob:String,token:Option<&str>)->Result<Option<usize>,String>{
        let token=token.ok_or("write token required")?;let role=token_role(token).ok_or("invalid role-bound token")?;
        if role=="view"{return Err("view role is read-only".into())}
        let mut conn=self.connect()?;let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM rooms WHERE id=?1)",[room],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !exists {if role!="full"{return Err("full token required to create room".into())}tx.execute("INSERT INTO rooms(id,created_at) VALUES(?1,?2)",params![room,now_ms()]).map_err(|e|e.to_string())?;tx.execute("INSERT INTO room_tokens(room_id,role,token_hash) VALUES(?1,'full',?2)",params![room,token_hash(token)]).map_err(|e|e.to_string())?;}
        let authorized:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM room_tokens WHERE room_id=?1 AND role=?2 AND token_hash=?3)",params![room,role,token_hash(token)],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !authorized{return Err("unauthorized room write".into())}
        let count:i64=tx.query_row("SELECT count(*) FROM events WHERE room_id=?1",[room],|r|r.get(0)).map_err(|e|e.to_string())?;if count>=MAX_ROOM_EVENTS as i64{return Ok(None)}let seq=count+1;
        tx.execute("INSERT INTO events(room_id,seq,blob,created_at) VALUES(?1,?2,?3,?4)",params![room,seq,blob,now_ms()]).map_err(|e|e.to_string())?;tx.commit().map_err(|e|e.to_string())?;Ok(Some(seq as usize))
    }
    pub fn get(&self,room:&str,since:usize)->(Vec<String>,usize){self.get_result(room,since).unwrap_or_default()}
    fn get_result(&self,room:&str,since:usize)->Result<(Vec<String>,usize),String>{let conn=self.connect()?;let mut stmt=conn.prepare("SELECT blob FROM events WHERE room_id=?1 AND seq>?2 ORDER BY seq").map_err(|e|e.to_string())?;let rows=stmt.query_map(params![room,since as i64],|r|r.get(0)).map_err(|e|e.to_string())?;let events=rows.collect::<Result<Vec<String>,_>>().map_err(|e|e.to_string())?;let next:i64=conn.query_row("SELECT coalesce(max(seq),0) FROM events WHERE room_id=?1",[room],|r|r.get(0)).map_err(|e|e.to_string())?;Ok((events,next as usize))}
    pub fn rotate_token(&self,room:&str,old:&str,new:&str)->Result<bool,String>{
        let old_role=token_role(old).ok_or("invalid old role token")?;let new_role=token_role(new).ok_or("invalid new role token")?;
        if old_role=="view"||new_role=="view"{return Ok(false)}
        let mut conn=self.connect()?;let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let authorized:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM room_tokens WHERE room_id=?1 AND role=?2 AND token_hash=?3)",params![room,old_role,token_hash(old)],|r|r.get(0)).map_err(|e|e.to_string())?;if !authorized{return Ok(false)}
        if old_role=="full"&&new_role!="full"{tx.execute("INSERT INTO room_tokens(room_id,role,token_hash) VALUES(?1,?2,?3) ON CONFLICT(room_id,role) DO UPDATE SET token_hash=excluded.token_hash,generation=room_tokens.generation+1",params![room,new_role,token_hash(new)]).map_err(|e|e.to_string())?;}else if old_role==new_role{tx.execute("UPDATE room_tokens SET token_hash=?3,generation=generation+1 WHERE room_id=?1 AND role=?2",params![room,old_role,token_hash(new)]).map_err(|e|e.to_string())?;}else{return Err("only full tokens may provision another role".into())}tx.commit().map_err(|e|e.to_string())?;Ok(true)
    }
    pub fn health(&self)->Result<serde_json::Value,String>{let conn=self.connect()?;let rooms:i64=conn.query_row("SELECT count(*) FROM rooms",[],|r|r.get(0)).map_err(|e|e.to_string())?;let events:i64=conn.query_row("SELECT count(*) FROM events",[],|r|r.get(0)).map_err(|e|e.to_string())?;let tokens:i64=conn.query_row("SELECT count(*) FROM room_tokens",[],|r|r.get(0)).map_err(|e|e.to_string())?;Ok(json!({"ok":true,"service":"jeden-collab-relay","backend":"sqlite-wal","contentBlind":true,"rooms":rooms,"events":events,"roleTokens":tokens,"path":self.path}))}
}

pub fn relay_response(store:&RelayStore,method:&str,path:&str,query:&str,body:&str)->(u16,String){relay_response_authorized(store,method,path,query,body,None)}
fn relay_response_authorized(store:&RelayStore,method:&str,path:&str,query:&str,body:&str,token:Option<&str>)->(u16,String){
 if method=="GET"&&path=="/health"{return match store.health(){Ok(v)=>(200,v.to_string()),Err(e)=>(500,json!({"ok":false,"error":e}).to_string())}}
 let target=match path.strip_prefix("/room/"){Some(r)if !r.is_empty()=>r,_=>return(404,json!({"ok":false,"error":"not found"}).to_string())};
 if method=="PUT"{if let Some(room)=target.strip_suffix("/token"){return match token{Some(old)=>match store.rotate_token(room,old,body.trim()){Ok(true)=>(200,json!({"ok":true}).to_string()),Ok(false)=>(403,json!({"ok":false,"error":"unauthorized"}).to_string()),Err(e)=>(400,json!({"ok":false,"error":e}).to_string())},None=>(403,json!({"ok":false,"error":"write token required"}).to_string())}}}
 match method{"POST"=>{let blob=body.trim();if blob.is_empty(){return(400,json!({"ok":false,"error":"empty body"}).to_string())}if blob.len()>MAX_BLOB_BYTES{return(413,json!({"ok":false,"error":"payload too large"}).to_string())}match store.post_authorized(target,blob.to_string(),token){Ok(Some(seq))=>(200,json!({"ok":true,"seq":seq}).to_string()),Ok(None)=>(429,json!({"ok":false,"error":"room is full"}).to_string()),Err(e)=>(403,json!({"ok":false,"error":e}).to_string())}},"GET"=>{let(events,next)=store.get(target,parse_since(query));(200,json!({"ok":true,"events":events,"next":next}).to_string())},_=>(405,json!({"ok":false,"error":"method not allowed"}).to_string())}
}
fn parse_since(query:&str)->usize{query.split('&').find_map(|p|p.strip_prefix("since=")).and_then(|v|v.parse().ok()).unwrap_or_default()}
pub fn serve(addr:&str)->Result<(),String>{let listener=TcpListener::bind(addr).map_err(|e|format!("bind {addr}: {e}"))?;let bound=listener.local_addr().map_err(|e|e.to_string())?;let store=std::sync::Arc::new(RelayStore::new());println!("jeden collab-relay listening on http://{bound} (durable {})",store.path().display());for stream in listener.incoming(){if let Ok(stream)=stream{let store=store.clone();std::thread::spawn(move||{let _=handle_conn(stream,&store);});}}Ok(())}
fn handle_conn(mut stream:TcpStream,store:&RelayStore)->std::io::Result<()>{let mut buf=Vec::new();let mut chunk=[0u8;4096];let header_end=loop{if let Some(pos)=find_subsequence(&buf,b"\r\n\r\n"){break pos+4}let n=stream.read(&mut chunk)?;if n==0{break buf.len()}buf.extend_from_slice(&chunk[..n]);if buf.len()>64*1024{break buf.len()}};let header=String::from_utf8_lossy(&buf[..header_end.min(buf.len())]).to_string();let(method,path,query)=parse_request_line(&header);let length=parse_content_length(&header);if length>MAX_BLOB_BYTES{write_response(&mut stream,413,&json!({"ok":false,"error":"payload too large"}).to_string())?;return Ok(())}while buf.len()<header_end+length{let n=stream.read(&mut chunk)?;if n==0{break}buf.extend_from_slice(&chunk[..n])}let body=String::from_utf8_lossy(&buf[header_end.min(buf.len())..]).to_string();let token=header_value(&header,"x-jeden-write-token");let requested_role=header_value(&header,"x-jeden-role");let(status,response)=match(token.as_deref(),requested_role.as_deref()){(Some(token),Some(role))if token_role(token)!=Some(role)=>(403,json!({"ok":false,"error":"write token is not valid for requested role"}).to_string()),_=>relay_response_authorized(store,&method,&path,&query,&body,token.as_deref())};write_response(&mut stream,status,&response)}
fn write_response(stream:&mut TcpStream,status:u16,body:&str)->std::io::Result<()>{let reason=match status{200=>"OK",400=>"Bad Request",403=>"Forbidden",404=>"Not Found",405=>"Method Not Allowed",413=>"Payload Too Large",429=>"Too Many Requests",_=>"Error"};let response=format!("HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type,x-jeden-write-token,x-jeden-role\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",body.len(),body);stream.write_all(response.as_bytes())?;stream.flush()}
fn find_subsequence(h:&[u8],n:&[u8])->Option<usize>{h.windows(n.len()).position(|w|w==n)}
pub(super)fn parse_request_line(header:&str)->(String,String,String){let mut p=header.lines().next().unwrap_or("").split_whitespace();let method=p.next().unwrap_or("").to_string();let target=p.next().unwrap_or("");let(path,query)=target.split_once('?').unwrap_or((target,""));(method,path.to_string(),query.to_string())}
pub(super)fn parse_content_length(header:&str)->usize{header_value(header,"content-length").and_then(|v|v.parse().ok()).unwrap_or_default()}
fn header_value(header:&str,name:&str)->Option<String>{header.lines().find_map(|line|{let(k,v)=line.split_once(':')?;k.trim().eq_ignore_ascii_case(name).then(||v.trim().to_string())})}
fn token_role(token:&str)->Option<&str>{let(role,_)=token.split_once('.')?;matches!(role,"view"|"prompt"|"abort"|"full").then_some(role)}
fn token_hash(token:&str)->String{hex::encode(Sha256::digest(token.as_bytes()))}
fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis().min(i64::MAX as u128)as i64}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::{new_role_write_token, open_frame, seal_frame, CollabRole, FrameKind, ProtocolFrame};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        dir: PathBuf,
        db: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "jeden-collab-runtime-{name}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let db = dir.join("relay.sqlite3");
            Self { dir, db }
        }

        fn store(&self) -> RelayStore {
            RelayStore::open(&self.db).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn roundtrip_http(store: &RelayStore, request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let (server, _) = listener.accept().unwrap();
        handle_conn(server, store).unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn collab_runtime_three_role_tokens_authorize_only_their_provisioned_role_and_mismatch_is_denied() {
        let fixture = Fixture::new("role-tokens");
        let store = fixture.store();
        let full = new_role_write_token(CollabRole::Full);
        let prompt = new_role_write_token(CollabRole::Prompt);
        let abort = new_role_write_token(CollabRole::Abort);
        assert_eq!(store.post_authorized("room", "full-event".into(), Some(&full)).unwrap(), Some(1));
        assert!(store.rotate_token("room", &full, &prompt).unwrap());
        assert!(store.rotate_token("room", &full, &abort).unwrap());
        assert_eq!(store.post_authorized("room", "prompt-event".into(), Some(&prompt)).unwrap(), Some(2));
        assert_eq!(store.post_authorized("room", "abort-event".into(), Some(&abort)).unwrap(), Some(3));

        let body = "must-not-persist";
        let request = format!(
            "POST /room/room HTTP/1.1\r\nhost: localhost\r\nx-jeden-write-token: {prompt}\r\nx-jeden-role: abort\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let response = roundtrip_http(&store, &request);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{response}");
        assert!(response.contains("write token is not valid for requested role"), "{response}");
        assert_eq!(store.get("room", 0).0, vec!["full-event", "prompt-event", "abort-event"]);

        assert!(store.post_authorized("room", "wrong-room-role".into(), Some(&new_role_write_token(CollabRole::Prompt))).is_err());
    }

    #[test]
    fn collab_runtime_view_replays_but_cannot_write_or_rotate_tokens() {
        let fixture = Fixture::new("view-role");
        let store = fixture.store();
        let full = new_role_write_token(CollabRole::Full);
        let view = new_role_write_token(CollabRole::View);
        assert_eq!(store.post_authorized("view-room", "seed-event".into(), Some(&full)).unwrap(), Some(1));

        let get = format!(
            "GET /room/view-room?since=0 HTTP/1.1\r\nhost: localhost\r\nx-jeden-write-token: {view}\r\nx-jeden-role: view\r\n\r\n"
        );
        let response = roundtrip_http(&store, &get);
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        let body: serde_json::Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["events"], serde_json::json!(["seed-event"]));
        assert_eq!(body["next"], 1);

        let blocked_event = "must-not-persist";
        let post = format!(
            "POST /room/view-room HTTP/1.1\r\nhost: localhost\r\nx-jeden-write-token: {view}\r\nx-jeden-role: view\r\ncontent-length: {}\r\n\r\n{blocked_event}",
            blocked_event.len()
        );
        let response = roundtrip_http(&store, &post);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{response}");
        assert!(response.contains("view role is read-only"), "{response}");
        assert_eq!(store.get("view-room", 0), (vec!["seed-event".into()], 1));

        let replacement_full = new_role_write_token(CollabRole::Full);
        let rotate_from_view = format!(
            "PUT /room/view-room/token HTTP/1.1\r\nhost: localhost\r\nx-jeden-write-token: {view}\r\nx-jeden-role: view\r\ncontent-length: {}\r\n\r\n{replacement_full}",
            replacement_full.len()
        );
        let response = roundtrip_http(&store, &rotate_from_view);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{response}");
        assert!(response.contains("unauthorized"), "{response}");

        let provision_view = format!(
            "PUT /room/view-room/token HTTP/1.1\r\nhost: localhost\r\nx-jeden-write-token: {full}\r\nx-jeden-role: full\r\ncontent-length: {}\r\n\r\n{view}",
            view.len()
        );
        let response = roundtrip_http(&store, &provision_view);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{response}");
        assert!(response.contains("unauthorized"), "{response}");

        let mismatched_role = format!(
            "GET /room/view-room HTTP/1.1\r\nhost: localhost\r\nx-jeden-write-token: {view}\r\nx-jeden-role: full\r\n\r\n"
        );
        let response = roundtrip_http(&store, &mismatched_role);
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{response}");
        assert!(response.contains("write token is not valid for requested role"), "{response}");
    }

    #[test]
    fn collab_runtime_e2ee_relay_persists_no_plaintext_or_key_material() {
        let fixture = Fixture::new("opaque-storage");
        let store = fixture.store();
        let key = [0x5a; 32];
        let token = new_role_write_token(CollabRole::Full);
        let secret = "launch code glacier-orchid-7391";
        let frame = ProtocolFrame::new(
            "host",
            CollabRole::Full,
            FrameKind::State { value: serde_json::json!({"secret": secret}) },
        )
        .unwrap();
        let blob = seal_frame(&key, &frame).unwrap();
        assert_eq!(store.post_authorized("secure-room", blob, Some(&token)).unwrap(), Some(1));

        let conn = Connection::open(&fixture.db).unwrap();
        let persisted_blob: String = conn.query_row("SELECT blob FROM events WHERE room_id='secure-room'", [], |row| row.get(0)).unwrap();
        let persisted_token_hash: String = conn.query_row("SELECT token_hash FROM room_tokens WHERE room_id='secure-room'", [], |row| row.get(0)).unwrap();
        assert!(!persisted_blob.contains(secret));
        assert!(!persisted_blob.contains(&hex::encode(key)));
        assert!(!persisted_token_hash.contains(&token));
        assert_eq!(open_frame(&key, &persisted_blob).unwrap(), frame);
        assert!(open_frame(&[0x5b; 32], &persisted_blob).is_err());
    }

    #[test]
    fn collab_runtime_typed_replay_is_ordered_and_restart_cursor_resumes_after_last_seen_event() {
        let fixture = Fixture::new("restart-replay");
        let store = fixture.store();
        let key = [0x2d; 32];
        let full = new_role_write_token(CollabRole::Full);
        let frames = vec![
            ProtocolFrame::new("host", CollabRole::Full, FrameKind::State { value: serde_json::json!({"step": 1}) }).unwrap(),
            ProtocolFrame::new("host", CollabRole::Full, FrameKind::Tool { tool_call_id: "call-2".into(), value: serde_json::json!({"step": 2}) }).unwrap(),
            ProtocolFrame::new("host", CollabRole::Full, FrameKind::Agent { agent_id: "worker-3".into(), value: serde_json::json!({"step": 3}) }).unwrap(),
        ];
        for (index, frame) in frames.iter().enumerate() {
            let seq = store.post_authorized("replay-room", seal_frame(&key, frame).unwrap(), Some(&full)).unwrap();
            assert_eq!(seq, Some(index + 1));
        }

        let (initial_blobs, cursor) = store.get("replay-room", 0);
        assert_eq!(cursor, 3);
        let initial = initial_blobs.iter().map(|blob| open_frame(&key, blob).unwrap()).collect::<Vec<_>>();
        assert_eq!(initial, frames);
        drop(store);

        let restarted = fixture.store();
        let (none, unchanged_cursor) = restarted.get("replay-room", cursor);
        assert!(none.is_empty());
        assert_eq!(unchanged_cursor, cursor);
        let fourth = ProtocolFrame::new("host", CollabRole::Full, FrameKind::State { value: serde_json::json!({"step": 4}) }).unwrap();
        assert_eq!(restarted.post_authorized("replay-room", seal_frame(&key, &fourth).unwrap(), Some(&full)).unwrap(), Some(4));
        let (resumed_blobs, resumed_cursor) = restarted.get("replay-room", cursor);
        assert_eq!(resumed_cursor, 4);
        assert_eq!(resumed_blobs.len(), 1);
        assert_eq!(open_frame(&key, &resumed_blobs[0]).unwrap(), fourth);
    }
}
