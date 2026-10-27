use crate::collab::CollabRole;
use crate::fleet::{run_db, sql};
use crate::tally::{FIRST, STEP};
use sea_orm::sea_query::{Expr, LockType, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, TransactionTrait,
};
use serde_json::json;
mod auth;
mod entities;
mod http;

use auth::{now_ms, relay_response_authorized, token_hash, token_role};

/// A role token's first generation; each rotation adds one step.
const FIRST_GENERATION: i64 = FIRST as i64;
use entities::{event, room, token};
pub use http::serve;

/// The relay tables in the fleet database `jeden`, created by
/// `crate::fleet`'s migrator.
pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS relay_rooms (id TEXT PRIMARY KEY, created_at BIGINT NOT NULL);
    CREATE TABLE IF NOT EXISTS relay_room_tokens (room_id TEXT NOT NULL REFERENCES relay_rooms(id)
        ON DELETE CASCADE, role TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
        generation BIGINT NOT NULL DEFAULT 1, PRIMARY KEY(room_id,role));
    CREATE TABLE IF NOT EXISTS relay_events (room_id TEXT NOT NULL REFERENCES relay_rooms(id)
        ON DELETE CASCADE, seq BIGINT NOT NULL, blob TEXT NOT NULL, created_at BIGINT NOT NULL,
        PRIMARY KEY(room_id,seq));";

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
    ) -> Result<usize, String> {
        let token = token.ok_or("write token required")?;
        let role = token_role(token).ok_or("invalid role-bound token")?;
        if role == CollabRole::View {
            return Err("view role is read-only".into());
        }
        let (room, hash) = (room.to_owned(), token_hash(token));
        let now = now_ms()?;
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            // Writers of one room serialize on its row (SELECT … FOR UPDATE).
            let exists = room::Entity::find_by_id(room.clone())
                .lock(LockType::Update)
                .one(&tx)
                .await
                .map_err(sql)?
                .is_some();
            if !exists {
                if role != CollabRole::Full {
                    return Err("full token required to create room".into());
                }
                room::Entity::insert(room::ActiveModel {
                    id: Set(room.clone()),
                    created_at: Set(now),
                })
                .exec_without_returning(&tx)
                .await
                .map_err(sql)?;
                token::Entity::insert(token::ActiveModel {
                    room_id: Set(room.clone()),
                    role: Set(CollabRole::Full.name()),
                    token_hash: Set(hash.clone()),
                    generation: Set(FIRST_GENERATION),
                })
                .exec_without_returning(&tx)
                .await
                .map_err(sql)?;
            }
            let authorized = token::Entity::find()
                .filter(token::Column::RoomId.eq(room.clone()))
                .filter(token::Column::Role.eq(role.name()))
                .filter(token::Column::TokenHash.eq(hash))
                .one(&tx)
                .await
                .map_err(sql)?
                .is_some();
            if !authorized {
                return Err("unauthorized room write".into());
            }
            let count = event::Entity::find()
                .filter(event::Column::RoomId.eq(room.clone()))
                .count(&tx)
                .await
                .map_err(sql)?;
            // The event takes the place after the room's last one.
            let seq = i64::try_from(count)
                .map_err(|error| format!("the room holds more events than the database counts: {error}"))?
                .checked_add(i64::from(STEP))
                .ok_or("collaboration event sequence exceeds the database integer range")?;
            let response_seq = usize::try_from(seq).map_err(|error| {
                format!("collaboration event sequence exceeds the host address range: {error}")
            })?;
            event::Entity::insert(event::ActiveModel {
                room_id: Set(room),
                seq: Set(seq),
                blob: Set(blob),
                created_at: Set(now),
            })
            .exec_without_returning(&tx)
            .await
            .map_err(sql)?;
            tx.commit().await.map_err(sql)?;
            Ok(response_seq)
        })
    }
    /// The room's events after `since` (all of them without one), and the
    /// sequence of its last event, none for a room without events.
    pub fn get(
        &self,
        room: &str,
        since: Option<usize>,
    ) -> Result<(Vec<String>, Option<usize>), String> {
        let room = room.to_owned();
        let since = match since.map(i64::try_from) {
            Some(Ok(since)) => Some(since),
            Some(Err(error)) => return Err(format!("the cursor is beyond the database integer range: {error}")),
            None => None,
        };
        run_db(move |db| async move {
            let mut events = event::Entity::find().filter(event::Column::RoomId.eq(room.clone()));
            if let Some(since) = since {
                events = events.filter(event::Column::Seq.gt(since));
            }
            let blobs = events
                .order_by_asc(event::Column::Seq)
                .all(&db)
                .await
                .map_err(sql)?
                .into_iter()
                .map(|event| event.blob)
                .collect();
            let next: Option<i64> = event::Entity::find()
                .select_only()
                .column_as(event::Column::Seq.max(), "next")
                .filter(event::Column::RoomId.eq(room))
                .into_tuple()
                .one(&db)
                .await
                .map_err(sql)?
                .flatten();
            let next = match next.map(usize::try_from) {
                Some(Ok(next)) => Some(next),
                Some(Err(error)) => return Err(format!("the room's last sequence does not fit this host: {error}")),
                None => None,
            };
            Ok((blobs, next))
        })
    }
    pub fn rotate_token(&self, room: &str, old: &str, new: &str) -> Result<bool, String> {
        let old_role = token_role(old).ok_or("invalid old role token")?;
        let new_role = token_role(new).ok_or("invalid new role token")?;
        if old_role == CollabRole::View || new_role == CollabRole::View {
            return Ok(false);
        }
        let (room, old_hash, new_hash) = (room.to_owned(), token_hash(old), token_hash(new));
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let authorized = token::Entity::find()
                .filter(token::Column::RoomId.eq(room.clone()))
                .filter(token::Column::Role.eq(old_role.name()))
                .filter(token::Column::TokenHash.eq(old_hash))
                .one(&tx)
                .await
                .map_err(sql)?
                .is_some();
            if !authorized {
                return Ok(false);
            }
            if old_role == CollabRole::Full && new_role != CollabRole::Full {
                token::Entity::insert(token::ActiveModel {
                    room_id: Set(room),
                    role: Set(new_role.name()),
                    token_hash: Set(new_hash),
                    generation: Set(FIRST_GENERATION),
                })
                .on_conflict(
                    OnConflict::columns([token::Column::RoomId, token::Column::Role])
                        .update_column(token::Column::TokenHash)
                        .value(
                            token::Column::Generation,
                            Expr::col((token::Entity, token::Column::Generation)).add(i64::from(STEP)),
                        )
                        .to_owned(),
                )
                .exec_without_returning(&tx)
                .await
                .map_err(sql)?;
            } else if old_role == new_role {
                token::Entity::update_many()
                    .col_expr(token::Column::TokenHash, Expr::value(new_hash))
                    .col_expr(
                        token::Column::Generation,
                        Expr::col(token::Column::Generation).add(i64::from(STEP)),
                    )
                    .filter(token::Column::RoomId.eq(room))
                    .filter(token::Column::Role.eq(old_role.name()))
                    .exec(&tx)
                    .await
                    .map_err(sql)?;
            } else {
                return Err("only full tokens may provision another role".into());
            }
            tx.commit().await.map_err(sql)?;
            Ok(true)
        })
    }
    pub fn health(&self) -> Result<serde_json::Value, String> {
        let (rooms, events, tokens) = run_db(|db| async move {
            Ok((
                room::Entity::find().count(&db).await.map_err(sql)?,
                event::Entity::find().count(&db).await.map_err(sql)?,
                token::Entity::find().count(&db).await.map_err(sql)?,
            ))
        })?;
        Ok(
            json!({"ok":true,"service":"jeden-collab-relay","backend":"fleet-postgres","contentBlind":true,"rooms":rooms,"events":events,"roleTokens":tokens,"location":self.location()}),
        )
    }
}
