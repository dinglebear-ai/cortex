-- =============================================================================
-- cortex: Current Production DB Schema (Baseline Contract)
-- =============================================================================
--
-- Pinning
--   This file pins the current production DB schema as of commit 6640f5d
--   (branch `main`, 2026-05-16). Source: `src/db/pool.rs` `init_pool()` plus
--   migrations 1..9. The production deployment on `nashost` operates at
--   ~4.9M log rows against this exact schema.
--
--   `db-additions.sql` LAYERS ONTO this baseline. Changing this file is a
--   major version event — coordinate with downstream consumers (CLI, MCP
--   schema, plugin skills, agent transport, RAG embed pipeline, alert
--   evaluator) before editing. Adding new optional columns to `logs` may be
--   non-breaking for readers but IS breaking for the `LogEntry` /
--   `LogEntryWithRaw` MCP response shapes — bump the minor version.
--
-- Graph projection addendum
--   Migration 27 adds graph_* projection tables for entity/relationship
--   lookup. These tables are rebuildable derived state over authoritative
--   sources (`logs`, heartbeats, inventory, error signatures, AI session
--   rollups). They intentionally do not mutate `logs` and do not use ingest
--   triggers. Source references are soft references because this process does
--   not enable PRAGMA foreign_keys on pooled SQLite connections.
--
-- FTS5 trigger invariant (load-bearing — DO NOT regress)
--   `logs_fts` has an INSERT-only trigger by design. DELETE / UPDATE on
--   `logs` do NOT propagate to the FTS index. This is intentional: bulk
--   DELETE during retention purge and storage-budget enforcement would fire
--   the trigger for every deleted row inside a single implicit transaction,
--   holding the SQLite write lock long enough to starve the batch writer and
--   drop UDP syslog at the kernel buffer. Phantom rows in `logs_fts` (where
--   the corresponding `logs.id` no longer exists) are skipped at query time
--   and cleaned up by SQLite's periodic incremental merge (merge=500,250).
--   See `src/db/pool.rs` line 84-93 and migration 1 (which DROPs `logs_ad`
--   and `logs_au` triggers on already-deployed databases).
--
-- Style
--   - Statements are arranged in dependency order so the file executes
--     top-to-bottom against a fresh database without forward references.
--   - `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` mirrors
--     the conventions used in `src/db/pool.rs`.
--   - RFC 3339 timestamps stored as TEXT with
--     `DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))` per the existing
--     conventions.
--   - Migration provenance is annotated above each migration-added column /
--     index in SQL comments.
-- =============================================================================


-- =============================================================================
-- 1. PRAGMAs (set on every pool connection via `configure_connection_pragmas`)
-- =============================================================================
-- Source: `src/db/pool.rs::configure_connection_pragmas` + `init_pool` setup.
-- These run on EVERY connection acquired from the r2d2 pool. Treat them as
-- part of the schema contract — disabling WAL, lowering busy_timeout, or
-- regressing auto_vacuum to NONE would break ingest under load and risk
-- unbounded growth.

PRAGMA journal_mode = WAL;            -- enabled when StorageConfig.wal_mode = true (default)
PRAGMA synchronous = NORMAL;          -- safe with WAL; faster than FULL
PRAGMA busy_timeout = 5000;           -- 5s wait before SQLITE_BUSY
PRAGMA cache_size = -64000;           -- 64 MiB per-connection page cache
PRAGMA auto_vacuum = INCREMENTAL;     -- one-shot at init; VACUUM run if pages > 0
                                      -- when switching from a different mode.

-- Not set at runtime but established by SQLite defaults / migration intent:
--   PRAGMA foreign_keys is NOT explicitly enabled. Foreign key references in
--   transcript_import_records / transcript_parse_errors are declarative only;
--   referential integrity is enforced by application code, not SQLite.


-- =============================================================================
-- 2. Tables
-- =============================================================================

-- ---------------------------------------------------------------------------
-- 2.1 `schema_migrations` — versioned migration ledger
-- Source: init_pool() (`src/db/pool.rs` ~L105). Guards each migration so it
-- runs exactly once per database, not on every startup.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);


-- ---------------------------------------------------------------------------
-- 2.1.1 `graph_*` — derived investigation graph projection (migrations 27, 30)
-- Source: init_pool() migration 27 plus migration 30 vocabulary widening.
--
-- Invariants:
--   - Rebuildable projection only; raw source tables remain authoritative.
--   - No `logs` mutation and no graph-maintenance triggers on ingest.
--   - `same_window` is deliberately absent from persisted v1 relationships.
--   - Source refs are allowlisted soft references, never free-form dynamic SQL
--     table names.
--   - Evidence is deduplicated/capped by `evidence_key`; relationship rows hold
--     aggregate counts and first/last seen fields.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS graph_entities (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Migration 41 (entity_resolution_v2): adds 'logical_service' and
    -- 'service_instance'. Canonical service identity is
    -- `logical_service:plex` + `service_instance:nashost/plex`; legacy
    -- 'service' rows ('nashost:plex', 'nashost:plex:plex') are deleted by the
    -- migration and no longer projected.
    entity_type   TEXT NOT NULL CHECK (entity_type IN (
        'host', 'container', 'service', 'app', 'source_ip',
        'ai_project', 'ai_session', 'error_signature', 'compose_project',
        'reverse_proxy', 'domain', 'network', 'storage', 'config_artifact',
        'git_commit', 'user', 'device', 'logical_service', 'service_instance'
    )),
    canonical_key TEXT NOT NULL,
    display_label TEXT NOT NULL,
    source_kind   TEXT NOT NULL DEFAULT '',
    source_id     TEXT NOT NULL DEFAULT '',
    trust_level   TEXT NOT NULL CHECK (trust_level IN (
        'verified', 'claimed', 'inferred', 'correlated', 'refuted'
    )),
    first_seen_at TEXT,
    last_seen_at  TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(entity_type, canonical_key)
);
CREATE INDEX IF NOT EXISTS idx_graph_entities_type_key
    ON graph_entities(entity_type, canonical_key);
CREATE INDEX IF NOT EXISTS idx_graph_entities_canonical_key
    ON graph_entities(canonical_key);

CREATE TABLE IF NOT EXISTS graph_entity_aliases (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id     INTEGER NOT NULL,
    alias_type    TEXT NOT NULL,
    alias_key     TEXT NOT NULL,
    alias_value   TEXT NOT NULL,
    source_kind   TEXT NOT NULL DEFAULT '',
    -- Migration 42 adds 'refuted' (missed by migrations 35/41 on the other
    -- three graph tables).
    trust_level   TEXT NOT NULL CHECK (trust_level IN (
        'verified', 'claimed', 'inferred', 'correlated', 'refuted'
    )),
    first_seen_at TEXT,
    last_seen_at  TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(entity_id, alias_type, alias_key, source_kind)
);
CREATE INDEX IF NOT EXISTS idx_graph_aliases_lookup
    ON graph_entity_aliases(alias_type, alias_key);
CREATE INDEX IF NOT EXISTS idx_graph_aliases_entity
    ON graph_entity_aliases(entity_id);

CREATE TABLE IF NOT EXISTS graph_relationships (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    relationship_key  TEXT NOT NULL UNIQUE,
    src_entity_id     INTEGER NOT NULL,
    dst_entity_id     INTEGER NOT NULL,
    -- Migration 41 adds 'instance_of' (service_instance -> logical_service)
    -- and the resolver_* reason codes.
    relationship_type TEXT NOT NULL CHECK (relationship_type IN (
        'observed_as', 'runs_on', 'emitted_by', 'worked_on',
        'matches_signature', 'defines_service', 'routes_to',
        'exposes_domain', 'attached_to', 'mounts', 'backed_by',
        'has_artifact', 'authenticated_as', 'accessed',
        'communicates_with', 'instance_of'
    )),
    reason_code       TEXT NOT NULL CHECK (reason_code IN (
        'syslog_claimed_hostname', 'log_app_name', 'docker_container_id',
        'docker_service_label', 'ai_session_project', 'heartbeat_host_state',
        'error_signature_match', 'inventory_node', 'inventory_service',
        'compose_config', 'reverse_proxy_config', 'docker_network',
        'storage_probe', 'config_artifact', 'agent_command_session',
        'agent_command_cwd_infer', 'agent_command_git_commit',
        'shell_history_git_commit', 'adguard_client_query',
        'shell_history_user', 'authelia_auth',
        'resolver_instance_of', 'resolver_service_instance',
        'resolver_raw_app_label'
    )),
    trust_level       TEXT NOT NULL CHECK (trust_level IN (
        'verified', 'claimed', 'inferred', 'correlated', 'refuted'
    )),
    confidence        REAL NOT NULL DEFAULT 0.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    evidence_count    INTEGER NOT NULL DEFAULT 0 CHECK (evidence_count >= 0),
    first_seen_at     TEXT,
    last_seen_at      TEXT,
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(src_entity_id, dst_entity_id, relationship_type, relationship_key)
);
CREATE INDEX IF NOT EXISTS idx_graph_relationships_src_type_seen
    ON graph_relationships(src_entity_id, relationship_type, last_seen_at DESC);
CREATE INDEX IF NOT EXISTS idx_graph_relationships_dst_type_seen
    ON graph_relationships(dst_entity_id, relationship_type, last_seen_at DESC);

CREATE TABLE IF NOT EXISTS graph_relationship_evidence (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    relationship_id       INTEGER NOT NULL,
    evidence_key          TEXT NOT NULL,
    source_kind           TEXT NOT NULL CHECK (source_kind IN (
        'log', 'heartbeat', 'ai_session_rollup', 'source_inventory',
        'app_inventory', 'error_signature'
    )),
    source_id             TEXT NOT NULL DEFAULT '',
    source_log_id         INTEGER,
    source_heartbeat_id   INTEGER,
    source_signature_hash TEXT,
    observed_at           TEXT NOT NULL,
    reason_code           TEXT NOT NULL CHECK (reason_code IN (
        'syslog_claimed_hostname', 'log_app_name', 'docker_container_id',
        'docker_service_label', 'ai_session_project', 'heartbeat_host_state',
        'error_signature_match', 'inventory_node', 'inventory_service',
        'compose_config', 'reverse_proxy_config', 'docker_network',
        'storage_probe', 'config_artifact', 'agent_command_session',
        'agent_command_cwd_infer', 'agent_command_git_commit',
        'shell_history_git_commit', 'adguard_client_query',
        'shell_history_user', 'authelia_auth',
        'resolver_instance_of', 'resolver_service_instance',
        'resolver_raw_app_label'
    )),
    reason_text           TEXT,
    confidence_delta      REAL NOT NULL DEFAULT 0.0 CHECK (confidence_delta >= -1.0 AND confidence_delta <= 1.0),
    trust_level           TEXT NOT NULL CHECK (trust_level IN (
        'verified', 'claimed', 'inferred', 'correlated', 'refuted'
    )),
    safe_excerpt          TEXT CHECK (safe_excerpt IS NULL OR length(safe_excerpt) <= 512),
    metadata_path         TEXT,
    evidence_count        INTEGER NOT NULL DEFAULT 1 CHECK (evidence_count >= 1),
    created_at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(relationship_id, evidence_key)
);
CREATE INDEX IF NOT EXISTS idx_graph_evidence_relationship_seen
    ON graph_relationship_evidence(relationship_id, observed_at DESC);
CREATE INDEX IF NOT EXISTS idx_graph_evidence_source_ref
    ON graph_relationship_evidence(source_kind, source_id);
CREATE INDEX IF NOT EXISTS idx_graph_evidence_log_id
    ON graph_relationship_evidence(source_log_id)
    WHERE source_log_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_graph_evidence_heartbeat_id
    ON graph_relationship_evidence(source_heartbeat_id)
    WHERE source_heartbeat_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS graph_projection_meta (
    id                 INTEGER PRIMARY KEY CHECK (id = 1),
    projection_status  TEXT NOT NULL CHECK (projection_status IN (
        'never_built', 'building', 'ready', 'stale', 'failed'
    )),
    last_started_at    TEXT,
    last_completed_at  TEXT,
    source_watermark   TEXT NOT NULL DEFAULT '',
    source_row_count   INTEGER NOT NULL DEFAULT 0 CHECK (source_row_count >= 0),
    entity_count       INTEGER NOT NULL DEFAULT 0 CHECK (entity_count >= 0),
    relationship_count INTEGER NOT NULL DEFAULT 0 CHECK (relationship_count >= 0),
    evidence_count     INTEGER NOT NULL DEFAULT 0 CHECK (evidence_count >= 0),
    is_degraded        INTEGER NOT NULL DEFAULT 0 CHECK (is_degraded IN (0, 1)),
    last_error         TEXT CHECK (last_error IS NULL OR length(last_error) <= 2048),
    last_runtime_ms    INTEGER NOT NULL DEFAULT 0 CHECK (last_runtime_ms >= 0),
    last_chunk_count   INTEGER NOT NULL DEFAULT 0 CHECK (last_chunk_count >= 0),
    updated_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    -- Migration 41: active graph projection contract identifier.
    projection_contract TEXT NOT NULL DEFAULT 'entity_resolution_v2'
);


-- ---------------------------------------------------------------------------
-- 2.2 `logs` — primary append-only event store
-- Source: init_pool() (`src/db/pool.rs` ~L46) plus column-add migrations.
--
-- Column provenance:
--   id, timestamp, hostname, facility, severity, app_name, process_id,
--   message, raw, received_at      → init_pool baseline
--   source_ip                       → init_pool baseline (pre-migration
--                                     fallback ALTER guarded by pragma_table_info
--                                     for DBs that predate the inline DEFAULT)
--   ai_tool, ai_project,
--   ai_session_id,
--   ai_transcript_path              → migration 4
--   metadata_json                   → migration 9
--
-- Invariants:
--   - Append-only in practice: no UPDATE codepaths exist today.
--   - `metadata_json` accepts a JSON object only (never an array, never a
--     scalar). Enforced by application code, not by CHECK constraint.
--   - `source_ip` carries network-verified identity for syslog (IP:port),
--     OTLP (peer IP), and Docker (`docker://host/container/stream` or
--     `docker-event://host/container/action`). Empty string '' is the
--     historical default for rows that predate verified identity.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS logs (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp          TEXT NOT NULL,
    hostname           TEXT NOT NULL,
    facility           TEXT,
    severity           TEXT NOT NULL,
    app_name           TEXT,
    process_id         TEXT,
    message            TEXT NOT NULL,
    raw                TEXT NOT NULL,
    received_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    source_ip          TEXT NOT NULL DEFAULT '',
    ai_tool            TEXT,                         -- migration 4
    ai_project         TEXT,                         -- migration 4
    ai_session_id      TEXT,                         -- migration 4
    ai_transcript_path TEXT,                         -- migration 4
    metadata_json      TEXT                          -- migration 9
);


-- ---------------------------------------------------------------------------
-- 2.3 `hosts` — hostname registry for quick lookups
-- Source: init_pool() (`src/db/pool.rs` ~L96).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS hosts (
    hostname   TEXT PRIMARY KEY,
    first_seen TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    log_count  INTEGER NOT NULL DEFAULT 0
);


-- ---------------------------------------------------------------------------
-- 2.4 `logs_fts` — FTS5 virtual table over `logs.message`
-- Source: init_pool() (`src/db/pool.rs` ~L77).
--
-- INVARIANT: this is a content-table FTS5 index over `logs(message)` with
-- rowid=id. The trigger below (`logs_ai`) is the SOLE write path. Bulk
-- DELETEs and UPDATEs intentionally do not propagate — see header note.
-- ---------------------------------------------------------------------------
CREATE VIRTUAL TABLE IF NOT EXISTS logs_fts USING fts5(
    message,
    content='logs',
    content_rowid='id',
    tokenize='porter unicode61'
);


-- ---------------------------------------------------------------------------
-- 2.5 `docker_ingest_checkpoints` — replay cursor for docker socket proxy
-- Source: migration 2 (`src/db/pool.rs` ~L162).
-- One row per (host, container). Lets short cortex outages replay from
-- Docker's local log store with `/containers/{id}/logs?since=`.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS docker_ingest_checkpoints (
    host_name      TEXT NOT NULL,
    container_id   TEXT NOT NULL,
    last_timestamp TEXT NOT NULL,
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (host_name, container_id)
);


-- ---------------------------------------------------------------------------
-- 2.6 `transcript_sources` — AI transcript file index
-- Source: migration 5 (`src/db/pool.rs` ~L262).
-- One row per discovered transcript file (Claude / Codex / Gemini). Tracks
-- byte offset for incremental tailing and the last error for retry triage.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS transcript_sources (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    canonical_path  TEXT    NOT NULL UNIQUE,
    source_kind     TEXT    NOT NULL,
    file_size       INTEGER,
    file_mtime      INTEGER,
    content_hash    TEXT,
    last_offset     INTEGER NOT NULL DEFAULT 0,
    last_indexed_at TEXT,
    last_error      TEXT
);


-- ---------------------------------------------------------------------------
-- 2.7 `transcript_import_records` — per-record import dedup ledger
-- Source: migration 6 (`src/db/pool.rs` ~L288).
-- Foreign key references `transcript_sources(id)` declaratively; not
-- enforced (PRAGMA foreign_keys is not enabled).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS transcript_import_records (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id   INTEGER NOT NULL REFERENCES transcript_sources(id),
    record_key  TEXT    NOT NULL,
    imported_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(source_id, record_key)
);


-- ---------------------------------------------------------------------------
-- 2.8 `transcript_parse_errors` — diagnostic ring for malformed transcript lines
-- Source: migration 7 (`src/db/pool.rs` ~L312).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS transcript_parse_errors (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id      INTEGER NOT NULL REFERENCES transcript_sources(id),
    line_no        INTEGER NOT NULL,
    error          TEXT    NOT NULL,
    record_preview TEXT    NOT NULL,
    seen_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(source_id, line_no, error, record_preview)
);


-- =============================================================================
-- 3. Triggers
-- =============================================================================

-- ---------------------------------------------------------------------------
-- 3.1 `logs_ai` — propagate inserts to FTS5
-- Source: init_pool() (`src/db/pool.rs` ~L91).
--
-- INVARIANT: this is the only trigger on `logs`. The `logs_ad` (AFTER DELETE)
-- and `logs_au` (AFTER UPDATE) triggers were removed by migration 1 because
-- their per-row firing during retention purges held the write lock long
-- enough to starve the batch writer. Do NOT add DELETE / UPDATE triggers.
-- ---------------------------------------------------------------------------
CREATE TRIGGER IF NOT EXISTS logs_ai AFTER INSERT ON logs BEGIN
    INSERT INTO logs_fts(rowid, message) VALUES (new.id, new.message);
END;


-- =============================================================================
-- 4. Indices
-- =============================================================================

-- ---------------------------------------------------------------------------
-- 4.1 Baseline indices on `logs` (init_pool, `src/db/pool.rs` ~L65)
-- ---------------------------------------------------------------------------
CREATE INDEX IF NOT EXISTS idx_logs_timestamp           ON logs(timestamp);
CREATE INDEX IF NOT EXISTS idx_logs_hostname            ON logs(hostname);
CREATE INDEX IF NOT EXISTS idx_logs_severity            ON logs(severity);
CREATE INDEX IF NOT EXISTS idx_logs_app_name            ON logs(app_name);
CREATE INDEX IF NOT EXISTS idx_logs_host_time           ON logs(hostname, timestamp);
CREATE INDEX IF NOT EXISTS idx_logs_sev_time            ON logs(severity, timestamp);
CREATE INDEX IF NOT EXISTS idx_logs_received_at         ON logs(received_at);
CREATE INDEX IF NOT EXISTS idx_logs_hostname_received_at
    ON logs(hostname, received_at);
CREATE INDEX IF NOT EXISTS idx_logs_source_ip_timestamp
    ON logs(source_ip, timestamp);

-- Dropped by init_pool() in favour of the composite source_ip+timestamp index.
-- Preserved here for completeness — operators upgrading from very old builds
-- will see this DROP fire exactly once.
-- DROP INDEX IF EXISTS idx_logs_source_ip;


-- ---------------------------------------------------------------------------
-- 4.2 Migration 3 — composite (app_name, received_at)
-- Source: migration 3 (`src/db/pool.rs` ~L201).
-- Supports `purge_by_tag_window` chunked DELETEs (e.g. all `adguard-allowed`
-- older than 7 days) without scanning the full app_name partition.
-- ---------------------------------------------------------------------------
CREATE INDEX IF NOT EXISTS idx_logs_app_name_received_at
    ON logs(app_name, received_at);


-- ---------------------------------------------------------------------------
-- 4.3 Migration 8 — partial AI metadata indexes
-- Source: migration 8 (`src/db/pool.rs` ~L340).
-- These supersede non-partial variants briefly created by migration 4. The
-- init_pool() tail (`src/db/pool.rs` ~L380) re-issues these CREATE INDEX IF
-- NOT EXISTS statements so fresh databases skip the DROP/CREATE dance.
-- ---------------------------------------------------------------------------
CREATE INDEX IF NOT EXISTS idx_logs_ai_project_time
    ON logs(ai_project, timestamp)
    WHERE ai_project IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_logs_ai_session
    ON logs(ai_tool, ai_project, ai_session_id)
    WHERE ai_tool IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_logs_ai_transcript_path
    ON logs(ai_transcript_path)
    WHERE ai_transcript_path IS NOT NULL;


-- ---------------------------------------------------------------------------
-- 4.4 Migration 6 / 7 — transcript-tables indices
-- Source: migrations 6 and 7 (`src/db/pool.rs` ~L295, ~L321).
-- ---------------------------------------------------------------------------
CREATE INDEX IF NOT EXISTS idx_transcript_import_records_source_id
    ON transcript_import_records(source_id);

CREATE INDEX IF NOT EXISTS idx_transcript_parse_errors_source_seen
    ON transcript_parse_errors(source_id, seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_transcript_parse_errors_seen
    ON transcript_parse_errors(seen_at DESC);


-- =============================================================================
-- 5. Schema Invariants
-- =============================================================================
-- These are not enforced by SQLite — they are enforced by application code
-- and code-review. Violating any of them is a regression.
--
--   I1. FTS5 INSERT-only trigger. `logs_ai` is the only trigger on `logs`.
--       Adding `AFTER DELETE` or `AFTER UPDATE` triggers reintroduces the
--       retention-purge starvation incident migration 1 was created to fix.
--
--   I2. `logs` is append-only in practice. There is no UPDATE codepath on
--       this table. Background sweepers DELETE old rows; ingest INSERTs new
--       rows. No reader assumes UPDATE-after-INSERT semantics.
--
--   I3. WAL mode. SQLite writes to `<db>.db`, `<db>.db-wal`, and
--       `<db>.db-shm`. All three files MUST be backed up together. Truncating
--       the WAL out-of-band corrupts in-flight transactions.
--
--   I4. `metadata_json` carries a JSON object only — never a top-level array,
--       never a scalar. Consumers SELECT it as TEXT and parse with
--       `serde_json::Value::Object`. Producers MUST round-trip through a
--       JSON-object serializer.
--
--   I5. `source_ip` is the only network-verified identity field. Hostname is
--       attacker-controlled on hostname-spoofable formats (e.g. UniFi CEF).
--       Filters that need spoof resistance MUST use `source_ip`.
--
--   I6. `auto_vacuum=INCREMENTAL` requires periodic `PRAGMA
--       incremental_vacuum(N)` to actually reclaim pages. The maintenance
--       module drives that — do NOT change auto_vacuum to NONE or FULL
--       without a coordinated maintenance change.
--
--   I7. `schema_migrations` is append-only. Re-running a migration version
--       is a corruption signal — the migration runner MUST skip already-
--       applied versions, never reapply.
-- =============================================================================
-- End of baseline contract.
-- =============================================================================
