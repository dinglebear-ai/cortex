-- Synthetic Cortex schema-43 upgrade fixture.
-- Generated from the migration contract in src/db/pool.rs, never from a live DB.
-- All values are deterministic and contain no host, user, credential, or secret data.

PRAGMA foreign_keys = OFF;

CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00.000Z'
);
WITH RECURSIVE versions(version) AS (
    SELECT 1
    UNION ALL
    SELECT version + 1 FROM versions WHERE version < 43
)
INSERT INTO schema_migrations(version, applied_at)
SELECT version, '2026-01-01T00:00:00.000Z' FROM versions;

-- Migration 37 is already marked applied in a schema-43 database, so its
-- durable audit table must be present even though this fixture keeps no LLM rows.
CREATE TABLE llm_invocations (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER,
    caller_surface TEXT NOT NULL,
    action TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT,
    program TEXT,
    incident_id TEXT,
    ai_tool TEXT,
    ai_project TEXT,
    ai_session_id TEXT,
    evidence_counts_json TEXT,
    prompt_bytes INTEGER,
    output_bytes INTEGER,
    status TEXT NOT NULL,
    error TEXT,
    metadata_json TEXT
);
CREATE INDEX idx_llm_invocations_started
    ON llm_invocations(started_at);
CREATE INDEX idx_llm_invocations_action_started
    ON llm_invocations(action, started_at);
CREATE INDEX idx_llm_invocations_status_started
    ON llm_invocations(status, started_at);

CREATE TABLE logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    hostname TEXT NOT NULL,
    facility TEXT,
    severity TEXT NOT NULL,
    app_name TEXT,
    process_id TEXT,
    message TEXT NOT NULL,
    raw TEXT NOT NULL,
    received_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00.000Z',
    source_ip TEXT NOT NULL DEFAULT '',
    ai_tool TEXT,
    ai_project TEXT,
    ai_session_id TEXT,
    ai_transcript_path TEXT,
    metadata_json TEXT
);
INSERT INTO logs (
    id, timestamp, hostname, facility, severity, app_name, process_id,
    message, raw, received_at, source_ip, ai_tool, ai_project,
    ai_session_id, ai_transcript_path, metadata_json
) VALUES (
    1,
    '2026-01-01T00:00:00.000Z',
    'fixture-host',
    'user',
    'info',
    'fixture-app',
    '1',
    'synthetic legacy log',
    '<14>synthetic legacy log',
    '2026-01-01T00:00:00.000Z',
    '192.0.2.1',
    'fixture-tool',
    'fixture-project',
    'fixture-session',
    'fixture://transcript/session.jsonl',
    '{"source_kind":"fixture"}'
);

CREATE TABLE ai_session_rollup (
    ai_project TEXT NOT NULL,
    ai_tool TEXT NOT NULL,
    ai_session_id TEXT NOT NULL,
    hostname TEXT NOT NULL,
    ai_transcript_path TEXT,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    event_count INTEGER NOT NULL,
    PRIMARY KEY (ai_project, ai_tool, ai_session_id, hostname)
);
CREATE INDEX idx_ai_session_rollup_last_seen
    ON ai_session_rollup(last_seen DESC);
INSERT INTO ai_session_rollup (
    ai_project, ai_tool, ai_session_id, hostname, ai_transcript_path,
    first_seen, last_seen, event_count
) VALUES (
    'fixture-project', 'fixture-tool', 'fixture-session', 'fixture-host',
    'fixture://transcript/session.jsonl',
    '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z', 1
);

CREATE TABLE ai_session_rollup_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    refreshed_at TEXT,
    row_count INTEGER NOT NULL DEFAULT 0,
    source_row_count INTEGER NOT NULL DEFAULT 0,
    source_max_id INTEGER NOT NULL DEFAULT 0
);
INSERT INTO ai_session_rollup_meta (
    id, refreshed_at, row_count, source_row_count, source_max_id
) VALUES (1, '2026-01-01T00:00:00.000Z', 1, 1, 1);

CREATE TABLE stream_last_seen (
    hostname TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    PRIMARY KEY (hostname, source_kind)
) WITHOUT ROWID;
INSERT INTO stream_last_seen(hostname, source_kind, last_seen_at)
VALUES ('fixture-host', 'fixture', '2026-01-01T00:00:00.000Z');
