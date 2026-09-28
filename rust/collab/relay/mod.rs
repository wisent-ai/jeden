use super::MAX_ROOM_EVENTS;
use crate::fleet::{run_db, sql};
use serde_json::json;
mod auth;
mod http;

use auth::{now_ms, relay_response_authorized, token_hash, token_role};
pub use http::serve;

/// The content-blind collab relay: rooms, their role tokens and their
/// encrypted events live in the fleet database `jeden` (tables `relay_*`),
/// so every relay process on every host serves the same rooms.
pub struct RelayStore;
impl RelayStore {
    pub fn new() -> Self {
        Self
    }
    /// Where the relay's rooms are kept, as the startup line shows it.
    pub fn location(&self) -> &'static str {
        "fleet database jeden (relay_rooms, relay_room_tokens, relay_events)"
    }
    pub fn post_authorized(
        &self,
        room: &str,
        blob: String,
        token: Option<&str>,
    ) -> Result<Option<usize>, String> {
        let token = token.ok_or("write token required")?;
        let role = token_role(token)
            .ok_or("invalid role-bound token")?
            .to_owned();
        if role == "view" {
            return Err("view role is read-only".into());
        }
        let (room, hash) = (room.to_owned(), token_hash(token));
        run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            // Serialize writers of one room on its row; a new room serializes
            // on the insert's primary key.
            let exists = tx
                .query_opt(
                    "SELECT id FROM relay_rooms WHERE id=$1 FOR UPDATE",
                    &[&room],
                )
                .map_err(sql)?
                .is_some();
            if !exists {
                if role != "full" {
                    return Err("full token required to create room".into());
                }
                tx.execute(
                    "INSERT INTO relay_rooms(id,created_at) VALUES($1,$2)",
                    &[&room, &now_ms()],
                )
                .map_err(sql)?;
                tx.execute(
                    "INSERT INTO relay_room_tokens(room_id,role,token_hash) VALUES($1,'full',$2)",
                    &[&room, &hash],
                )
                .map_err(sql)?;
            }
            let authorized: bool = tx
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM relay_room_tokens WHERE room_id=$1 AND role=$2 AND token_hash=$3)",
                    &[&room, &role, &hash],
                )
                .map_err(sql)?
                .get(0);
            if !authorized {
                return Err("unauthorized room write".into());
            }
            let count: i64 = tx
                .query_one(
                    "SELECT count(*) FROM relay_events WHERE room_id=$1",
                    &[&room],
                )
                .map_err(sql)?
                .get(0);
            if count >= MAX_ROOM_EVENTS as i64 {
                return Ok(None);
            }
            let seq = count + 1;
            tx.execute(
                "INSERT INTO relay_events(room_id,seq,blob,created_at) VALUES($1,$2,$3,$4)",
                &[&room, &seq, &blob, &now_ms()],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok(Some(seq as usize))
        })
    }
    pub fn get(&self, room: &str, since: usize) -> Result<(Vec<String>, usize), String> {
        let (room, since) = (room.to_owned(), since as i64);
        run_db(move |client| {
            let events = client
                .query(
                    "SELECT blob FROM relay_events WHERE room_id=$1 AND seq>$2 ORDER BY seq",
                    &[&room, &since],
                )
                .map_err(sql)?
                .into_iter()
                .map(|row| row.get(0))
                .collect();
            let next: i64 = client
                .query_one(
                    "SELECT coalesce(max(seq),0) FROM relay_events WHERE room_id=$1",
                    &[&room],
                )
                .map_err(sql)?
                .get(0);
            Ok((events, next as usize))
        })
    }
    pub fn rotate_token(&self, room: &str, old: &str, new: &str) -> Result<bool, String> {
        let old_role = token_role(old).ok_or("invalid old role token")?.to_owned();
        let new_role = token_role(new).ok_or("invalid new role token")?.to_owned();
        if old_role == "view" || new_role == "view" {
            return Ok(false);
        }
        let (room, old_hash, new_hash) = (room.to_owned(), token_hash(old), token_hash(new));
        run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            let authorized: bool = tx
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM relay_room_tokens WHERE room_id=$1 AND role=$2 AND token_hash=$3)",
                    &[&room, &old_role, &old_hash],
                )
                .map_err(sql)?
                .get(0);
            if !authorized {
                return Ok(false);
            }
            if old_role == "full" && new_role != "full" {
                tx.execute(
                    "INSERT INTO relay_room_tokens(room_id,role,token_hash) VALUES($1,$2,$3)
                     ON CONFLICT(room_id,role) DO UPDATE SET token_hash=excluded.token_hash,
                     generation=relay_room_tokens.generation+1",
                    &[&room, &new_role, &new_hash],
                )
                .map_err(sql)?;
            } else if old_role == new_role {
                tx.execute(
                    "UPDATE relay_room_tokens SET token_hash=$3,generation=generation+1
                     WHERE room_id=$1 AND role=$2",
                    &[&room, &old_role, &new_hash],
                )
                .map_err(sql)?;
            } else {
                return Err("only full tokens may provision another role".into());
            }
            tx.commit().map_err(sql)?;
            Ok(true)
        })
    }
    pub fn health(&self) -> Result<serde_json::Value, String> {
        let (rooms, events, tokens): (i64, i64, i64) = run_db(|client| {
            let row = client
                .query_one(
                    "SELECT (SELECT count(*) FROM relay_rooms), (SELECT count(*) FROM relay_events),
                            (SELECT count(*) FROM relay_room_tokens)",
                    &[],
                )
                .map_err(sql)?;
            Ok((row.get(0), row.get(1), row.get(2)))
        })?;
        Ok(
            json!({"ok":true,"service":"jeden-collab-relay","backend":"fleet-postgres","contentBlind":true,"rooms":rooms,"events":events,"roleTokens":tokens,"location":self.location()}),
        )
    }
}
