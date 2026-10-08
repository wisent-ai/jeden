//! The memory tables in the fleet database `jeden`. `crate::fleet` creates
//! them when it connects; the lexical index is a generated `tsvector`
//! column, so it can never fall out of step with the text it indexes.

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS memories (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, scope_kind TEXT NOT NULL, scope_id TEXT NOT NULL,
        text TEXT NOT NULL, tags_json TEXT NOT NULL, source_json TEXT NOT NULL,
        confidence DOUBLE PRECISION NOT NULL CHECK (confidence BETWEEN 0 AND 1),
        status TEXT NOT NULL, created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL,
        logical_key TEXT NOT NULL, revision BIGINT NOT NULL DEFAULT 1,
        valid_from BIGINT NOT NULL, valid_to BIGINT, supersedes TEXT REFERENCES memories(id),
        tombstone BOOLEAN NOT NULL DEFAULT FALSE,
        search TSVECTOR GENERATED ALWAYS AS
            (to_tsvector('simple'::regconfig, text || ' ' || tags_json || ' ' || kind)) STORED);
    CREATE INDEX IF NOT EXISTS memories_scope ON memories(scope_kind,scope_id,status,updated_at DESC);
    CREATE UNIQUE INDEX IF NOT EXISTS memories_revision ON memories(scope_kind,scope_id,logical_key,revision);
    CREATE INDEX IF NOT EXISTS memories_temporal
        ON memories(scope_kind,scope_id,valid_from,valid_to,tombstone,status);
    CREATE INDEX IF NOT EXISTS memories_search ON memories USING GIN (search);
    CREATE TABLE IF NOT EXISTS memory_jobs (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload_json TEXT NOT NULL, state TEXT NOT NULL,
        attempts BIGINT NOT NULL DEFAULT 0, available_at BIGINT NOT NULL, lease_owner TEXT,
        lease_until BIGINT, heartbeat_at BIGINT, last_error TEXT,
        created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL);
    CREATE INDEX IF NOT EXISTS memory_jobs_claim ON memory_jobs(state,available_at,lease_until);
    CREATE TABLE IF NOT EXISTS memory_workflows (
        fingerprint TEXT PRIMARY KEY, description TEXT NOT NULL, sessions_json TEXT NOT NULL,
        occurrences BIGINT NOT NULL, verified BOOLEAN NOT NULL DEFAULT FALSE,
        updated_at BIGINT NOT NULL);
    CREATE TABLE IF NOT EXISTS memory_managed_skills (
        id TEXT PRIMARY KEY,
        workflow_fingerprint TEXT NOT NULL UNIQUE REFERENCES memory_workflows(fingerprint),
        body TEXT NOT NULL, created_at BIGINT NOT NULL);
    CREATE TABLE IF NOT EXISTS memory_edges (
        from_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
        to_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
        relation TEXT NOT NULL CHECK (relation IN ('supports','conflicts','duplicates')),
        created_at BIGINT NOT NULL, provenance_json TEXT NOT NULL DEFAULT '{}',
        PRIMARY KEY(from_id,to_id,relation));
    CREATE INDEX IF NOT EXISTS memory_edges_to ON memory_edges(to_id,relation);
    CREATE TABLE IF NOT EXISTS memory_embeddings (
        memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
        model TEXT NOT NULL, dimensions BIGINT NOT NULL, vector_json TEXT NOT NULL,
        content_hash TEXT NOT NULL, updated_at BIGINT NOT NULL);
    CREATE TABLE IF NOT EXISTS memory_outbox (
        id TEXT PRIMARY KEY, dedupe_key TEXT NOT NULL UNIQUE, event_kind TEXT NOT NULL,
        payload_json TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending',
        attempts BIGINT NOT NULL DEFAULT 0, available_at BIGINT NOT NULL, lease_owner TEXT,
        lease_until BIGINT, last_error TEXT, created_at BIGINT NOT NULL, processed_at BIGINT);
    CREATE INDEX IF NOT EXISTS memory_outbox_claim ON memory_outbox(state,available_at,lease_until);
    CREATE TABLE IF NOT EXISTS memory_processed_events (
        consumer TEXT NOT NULL, event_id TEXT NOT NULL, processed_at BIGINT NOT NULL,
        PRIMARY KEY(consumer,event_id));";
