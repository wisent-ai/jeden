use super::MAX_ROOM_EVENTS;
use crate::fleet::{run_db, sql};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, TransactionTrait,
};
use serde_json::json;
mod auth;
mod entities;
mod http;

use auth::{now_ms, relay_response_authorized, token_hash, token_role};
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
    ) -> Result<Option<usize>, String> {
        let token = token.ok_or("write token required")?;
        let role = token_role(token)
            .ok_or("invalid role-bound token")?
            .to_owned();
        if role == "view" {
            return Err("view role is read-only".into());
        }
        let (room, hash) = (room.to_owned(), token_hash(token));
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            // Writers of one room serialize on its row.
            let exists = room::Entity::find_by_id(room.clone())
                .lock_exclusive()
                .one(&tx)
                .await
                .map_err(sql)?
                .is_some();
            if !exists {
                if role != "full" {
                    return Err("full token required to create room".into());
                }
                room::Entity::insert(room::ActiveModel {
                    id: Set(room.clone()),
                    created_at: Set(now_ms()),
                })
                .exec_without_returning(&tx)
                .await
                .map_err(sql)?;
                token::Entity::insert(token::ActiveModel {
                    room_id: Set(room.clone()),
                    role: Set("full".into()),
                    token_hash: Set(hash.clone()),
                    generation: Set(1),
                })
                .exec_without_returning(&tx)
                .await
                .map_err(sql)?;
            }
            let authorized = token::Entity::find()
                .filter(token::Column::RoomId.eq(room.clone()))
                .filter(token::Column::Role.eq(role))
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
            if count >= MAX_ROOM_EVENTS as u64 {
                return Ok(None);
            }
            let seq = count as i64 + 1;
            event::Entity::insert(event::ActiveModel {
                room_id: Set(room),
                seq: Set(seq),
                blob: Set(blob),
                created_at: Set(now_ms()),
            })
            .exec_without_returning(&tx)
            .await
            .map_err(sql)?;
            tx.commit().await.map_err(sql)?;
            Ok(Some(seq as usize))
        })
    }
    pub fn get(&self, room: &str, since: usize) -> Result<(Vec<String>, usize), String> {
        let (room, since) = (room.to_owned(), since as i64);
        run_db(move |db| async move {
            let blobs = event::Entity::find()
                .filter(event::Column::RoomId.eq(room.clone()))
                .filter(event::Column::Seq.gt(since))
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
            Ok((blobs, next.unwrap_or(0) as usize))
        })
    }
    pub fn rotate_token(&self, room: &str, old: &str, new: &str) -> Result<bool, String> {
        let old_role = token_role(old).ok_or("invalid old role token")?.to_owned();
        let new_role = token_role(new).ok_or("invalid new role token")?.to_owned();
        if old_role == "view" || new_role == "view" {
            return Ok(false);
        }
        let (room, old_hash, new_hash) = (room.to_owned(), token_hash(old), token_hash(new));
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let authorized = token::Entity::find()
                .filter(token::Column::RoomId.eq(room.clone()))
                .filter(token::Column::Role.eq(old_role.clone()))
                .filter(token::Column::TokenHash.eq(old_hash))
                .one(&tx)
                .await
                .map_err(sql)?
                .is_some();
            if !authorized {
                return Ok(false);
            }
            if old_role == "full" && new_role != "full" {
                token::Entity::insert(token::ActiveModel {
                    room_id: Set(room),
                    role: Set(new_role),
                    token_hash: Set(new_hash),
                    generation: Set(1),
                })
                .on_conflict(
                    OnConflict::columns([token::Column::RoomId, token::Column::Role])
                        .update_column(token::Column::TokenHash)
                        .value(
                            token::Column::Generation,
                            Expr::col((token::Entity, token::Column::Generation)).add(1),
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
                        Expr::col(token::Column::Generation).add(1),
                    )
                    .filter(token::Column::RoomId.eq(room))
                    .filter(token::Column::Role.eq(old_role))
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
