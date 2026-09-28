//! Memory's tables as SeaORM entities. The generated `search` column of
//! `memories` is left out: Postgres fills it, and only ranking reads it.

macro_rules! entity {
    ($module:ident, $table:literal, { $($body:tt)* }) => {
        pub(crate) mod $module {
            use sea_orm::entity::prelude::*;

            #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
            #[sea_orm(table_name = $table)]
            pub struct Model { $($body)* }
            #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
            pub enum Relation {}
            impl ActiveModelBehavior for ActiveModel {}
        }
    };
}

entity!(memory, "memories", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub kind: String,
    pub scope_kind: String,
    pub scope_id: String,
    pub text: String,
    pub tags_json: String,
    pub source_json: String,
    pub confidence: f64,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub logical_key: String,
    pub revision: i64,
    pub valid_from: i64,
    pub valid_to: Option<i64>,
    pub supersedes: Option<String>,
    pub tombstone: bool,
});

entity!(job, "memory_jobs", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub kind: String,
    pub payload_json: String,
    pub state: String,
    pub attempts: i64,
    pub available_at: i64,
    pub lease_owner: Option<String>,
    pub lease_until: Option<i64>,
    pub heartbeat_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
});

entity!(scope_lock, "memory_scope_locks", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope_kind: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope_id: String,
    pub owner: String,
    pub expires_at: i64,
});

entity!(workflow, "memory_workflows", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub fingerprint: String,
    pub description: String,
    pub sessions_json: String,
    pub occurrences: i64,
    pub verified: bool,
    pub updated_at: i64,
});

entity!(managed_skill, "memory_managed_skills", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub workflow_fingerprint: String,
    pub body: String,
    pub created_at: i64,
});

entity!(edge, "memory_edges", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub from_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub to_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub relation: String,
    pub created_at: i64,
    pub provenance_json: String,
});

entity!(embedding, "memory_embeddings", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub memory_id: String,
    pub model: String,
    pub dimensions: i64,
    pub vector_json: String,
    pub content_hash: String,
    pub updated_at: i64,
});

entity!(outbox, "memory_outbox", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub dedupe_key: String,
    pub event_kind: String,
    pub payload_json: String,
    pub state: String,
    pub attempts: i64,
    pub available_at: i64,
    pub lease_owner: Option<String>,
    pub lease_until: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub processed_at: Option<i64>,
});

entity!(processed_event, "memory_processed_events", {
    #[sea_orm(primary_key, auto_increment = false)]
    pub consumer: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub event_id: String,
    pub processed_at: i64,
});
