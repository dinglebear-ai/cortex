-- Cortex Agent Observatory planned additive schema.
-- Baseline: Cortex schema 43. Planned migrations: 44 through 47.
-- This file is a contract fixture. Production migrations belong in src/db/pool.rs.

PRAGMA foreign_keys = ON;

-- Migration 44: repository and Git topology.
CREATE TABLE repositories (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    repository_key      TEXT NOT NULL UNIQUE,
    hostname            TEXT NOT NULL,
    common_git_dir      TEXT NOT NULL,
    primary_path        TEXT NOT NULL,
    display_name        TEXT NOT NULL,
    remote_url_hash     TEXT,
    first_seen_at       TEXT NOT NULL,
    last_seen_at        TEXT NOT NULL,
    removed_at          TEXT,
    metadata_json       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(hostname, common_git_dir)
);

CREATE INDEX idx_repositories_host_seen
    ON repositories(hostname, last_seen_at DESC);
CREATE INDEX idx_repositories_display
    ON repositories(display_name COLLATE NOCASE);

CREATE TABLE repository_worktrees (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    worktree_key        TEXT NOT NULL UNIQUE,
    repository_id       INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    hostname            TEXT NOT NULL,
    path                TEXT NOT NULL,
    git_dir             TEXT NOT NULL,
    branch_ref          TEXT,
    branch_name         TEXT,
    head_sha            TEXT,
    upstream_ref        TEXT,
    detached            INTEGER NOT NULL DEFAULT 0 CHECK (detached IN (0, 1)),
    bare                INTEGER NOT NULL DEFAULT 0 CHECK (bare IN (0, 1)),
    locked              INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
    lock_reason         TEXT,
    prunable            INTEGER NOT NULL DEFAULT 0 CHECK (prunable IN (0, 1)),
    prune_reason        TEXT,
    dirty               INTEGER NOT NULL DEFAULT 0 CHECK (dirty IN (0, 1)),
    staged_count        INTEGER NOT NULL DEFAULT 0 CHECK (staged_count >= 0),
    unstaged_count      INTEGER NOT NULL DEFAULT 0 CHECK (unstaged_count >= 0),
    untracked_count     INTEGER NOT NULL DEFAULT 0 CHECK (untracked_count >= 0),
    ahead               INTEGER CHECK (ahead IS NULL OR ahead >= 0),
    behind              INTEGER CHECK (behind IS NULL OR behind >= 0),
    status_hash         TEXT,
    first_seen_at       TEXT NOT NULL,
    last_seen_at        TEXT NOT NULL,
    removed_at          TEXT,
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(hostname, path)
);

CREATE INDEX idx_worktrees_repo_active
    ON repository_worktrees(repository_id, removed_at, last_seen_at DESC);
CREATE INDEX idx_worktrees_branch
    ON repository_worktrees(branch_name, last_seen_at DESC);
CREATE INDEX idx_worktrees_head
    ON repository_worktrees(repository_id, head_sha);

CREATE TABLE repository_observations (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    observation_key     TEXT NOT NULL UNIQUE,
    repository_id       INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    worktree_id         INTEGER REFERENCES repository_worktrees(id) ON DELETE CASCADE,
    observed_at         TEXT NOT NULL,
    observation_kind    TEXT NOT NULL CHECK (observation_kind IN (
        'discovered', 'status', 'head', 'branch', 'worktree_added',
        'worktree_removed', 'overflow_reconcile', 'periodic_reconcile', 'error'
    )),
    old_head_sha        TEXT,
    new_head_sha        TEXT,
    summary             TEXT NOT NULL DEFAULT '',
    payload_json        TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)),
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_repository_observations_worktree_time
    ON repository_observations(worktree_id, observed_at DESC, id DESC);
CREATE INDEX idx_repository_observations_repo_time
    ON repository_observations(repository_id, observed_at DESC, id DESC);

CREATE TABLE git_commits (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    repository_id       INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    sha                 TEXT NOT NULL,
    parent_shas_json    TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(parent_shas_json)),
    author_name         TEXT,
    author_email_hash   TEXT,
    authored_at         TEXT,
    committed_at        TEXT,
    subject             TEXT NOT NULL DEFAULT '',
    changed_files       INTEGER CHECK (changed_files IS NULL OR changed_files >= 0),
    insertions          INTEGER CHECK (insertions IS NULL OR insertions >= 0),
    deletions           INTEGER CHECK (deletions IS NULL OR deletions >= 0),
    changed_paths_json  TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(changed_paths_json)),
    first_observed_at   TEXT NOT NULL,
    last_observed_at    TEXT NOT NULL,
    reachable           INTEGER NOT NULL DEFAULT 1 CHECK (reachable IN (0, 1)),
    metadata_json       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE(repository_id, sha)
);

CREATE INDEX idx_git_commits_repo_time
    ON git_commits(repository_id, committed_at DESC, id DESC);

-- Migration 45: durable run projection and stream outbox.
CREATE TABLE agent_runs (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    run_key                 TEXT NOT NULL UNIQUE,
    native_session_id       TEXT NOT NULL,
    tool                    TEXT NOT NULL,
    provider_tool           TEXT,
    hostname                TEXT NOT NULL,
    parent_run_id           INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
    previous_run_id         INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
    primary_worktree_id     INTEGER REFERENCES repository_worktrees(id) ON DELETE SET NULL,
    transcript_path         TEXT,
    process_id              TEXT,
    status                  TEXT NOT NULL CHECK (status IN (
        'starting', 'active', 'waiting', 'idle', 'stale',
        'completed', 'failed', 'abandoned'
    )),
    status_reason           TEXT NOT NULL DEFAULT '',
    status_observed_at      TEXT NOT NULL,
    started_at              TEXT NOT NULL,
    last_activity_at        TEXT NOT NULL,
    ended_at                TEXT,
    first_source_log_id     INTEGER,
    last_source_log_id      INTEGER,
    last_event_id           INTEGER,
    event_count             INTEGER NOT NULL DEFAULT 0 CHECK (event_count >= 0),
    error_count             INTEGER NOT NULL DEFAULT 0 CHECK (error_count >= 0),
    primary_branch          TEXT,
    start_head_sha          TEXT,
    current_head_sha        TEXT,
    projection_version      INTEGER NOT NULL DEFAULT 1,
    freshness_json          TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(freshness_json)),
    metadata_json           TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    created_at              TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at              TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(hostname, tool, native_session_id)
);

CREATE INDEX idx_agent_runs_activity
    ON agent_runs(last_activity_at DESC, id DESC);
CREATE INDEX idx_agent_runs_status_activity
    ON agent_runs(status, last_activity_at DESC, id DESC);
CREATE INDEX idx_agent_runs_worktree_activity
    ON agent_runs(primary_worktree_id, last_activity_at DESC, id DESC);
CREATE INDEX idx_agent_runs_tool_host
    ON agent_runs(tool, hostname, last_activity_at DESC);

CREATE TABLE agent_run_actors (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_key           TEXT NOT NULL UNIQUE,
    run_id              INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    native_actor_id     TEXT NOT NULL,
    actor_type          TEXT,
    display_name        TEXT,
    started_at          TEXT,
    last_activity_at    TEXT,
    ended_at            TEXT,
    metadata_json       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE(run_id, native_actor_id)
);

CREATE INDEX idx_agent_run_actors_run
    ON agent_run_actors(run_id, last_activity_at DESC);

CREATE TABLE agent_run_worktrees (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    relation_key        TEXT NOT NULL UNIQUE,
    run_id              INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    worktree_id         INTEGER NOT NULL REFERENCES repository_worktrees(id) ON DELETE CASCADE,
    evidence_kind       TEXT NOT NULL,
    evidence_source     TEXT NOT NULL,
    trust_level         TEXT NOT NULL CHECK (trust_level IN (
        'verified', 'claimed', 'correlated', 'inferred', 'refuted'
    )),
    confidence          REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    is_primary          INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1)),
    first_seen_at       TEXT NOT NULL,
    last_seen_at        TEXT NOT NULL,
    metadata_json       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE(run_id, worktree_id, evidence_kind, evidence_source)
);

CREATE INDEX idx_agent_run_worktrees_run
    ON agent_run_worktrees(run_id, is_primary DESC, confidence DESC, last_seen_at DESC);
CREATE INDEX idx_agent_run_worktrees_worktree
    ON agent_run_worktrees(worktree_id, last_seen_at DESC, run_id);

CREATE TABLE agent_run_events (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    event_key           TEXT NOT NULL UNIQUE,
    run_id              INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    actor_id            INTEGER REFERENCES agent_run_actors(id) ON DELETE SET NULL,
    worktree_id         INTEGER REFERENCES repository_worktrees(id) ON DELETE SET NULL,
    commit_id           INTEGER REFERENCES git_commits(id) ON DELETE SET NULL,
    observed_at         TEXT NOT NULL,
    ingested_at         TEXT NOT NULL,
    event_kind          TEXT NOT NULL CHECK (event_kind IN (
        'lifecycle', 'transcript', 'command', 'shell_history',
        'git_status', 'git_head', 'git_commit', 'file_operation',
        'mcp', 'hook', 'skill', 'llm', 'otlp_log', 'otlp_span',
        'otlp_metric', 'heartbeat', 'error', 'provider_event'
    )),
    source_kind         TEXT NOT NULL,
    source_id           TEXT NOT NULL,
    source_log_id       INTEGER,
    provider_sequence   INTEGER,
    trace_id            TEXT,
    span_id             TEXT,
    severity            TEXT NOT NULL DEFAULT 'info',
    title               TEXT NOT NULL DEFAULT '',
    summary             TEXT NOT NULL DEFAULT '',
    payload_json        TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)),
    content_scrubbed    INTEGER NOT NULL DEFAULT 1 CHECK (content_scrubbed IN (0, 1)),
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_agent_run_events_run_order
    ON agent_run_events(run_id, observed_at DESC, id DESC);
CREATE INDEX idx_agent_run_events_run_kind
    ON agent_run_events(run_id, event_kind, observed_at DESC, id DESC);
CREATE INDEX idx_agent_run_events_trace
    ON agent_run_events(trace_id, span_id);
CREATE INDEX idx_agent_run_events_source_log
    ON agent_run_events(source_log_id) WHERE source_log_id IS NOT NULL;

CREATE TABLE agent_run_commits (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    relation_key        TEXT NOT NULL UNIQUE,
    run_id              INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    commit_id           INTEGER NOT NULL REFERENCES git_commits(id) ON DELETE CASCADE,
    worktree_id         INTEGER REFERENCES repository_worktrees(id) ON DELETE SET NULL,
    evidence_kind       TEXT NOT NULL,
    evidence_source     TEXT NOT NULL,
    trust_level         TEXT NOT NULL CHECK (trust_level IN (
        'verified', 'claimed', 'correlated', 'inferred', 'refuted'
    )),
    confidence          REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    observed_at         TEXT NOT NULL,
    metadata_json       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE(run_id, commit_id, evidence_kind, evidence_source)
);

CREATE INDEX idx_agent_run_commits_run
    ON agent_run_commits(run_id, observed_at DESC, commit_id);

CREATE TABLE agent_projection_cursors (
    source_name         TEXT PRIMARY KEY,
    last_source_id      INTEGER NOT NULL DEFAULT 0 CHECK (last_source_id >= 0),
    source_max_id       INTEGER NOT NULL DEFAULT 0 CHECK (source_max_id >= 0),
    projection_version  INTEGER NOT NULL DEFAULT 1,
    last_success_at     TEXT,
    last_error_at       TEXT,
    last_error          TEXT,
    retry_count         INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE agent_stream_outbox (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    event_name          TEXT NOT NULL CHECK (event_name IN (
        'run.created', 'run.updated', 'run.status', 'run.event',
        'worktree.updated', 'repository.updated', 'telemetry.updated',
        'observatory.reset'
    )),
    entity_type         TEXT NOT NULL,
    entity_key          TEXT NOT NULL,
    run_id              INTEGER REFERENCES agent_runs(id) ON DELETE CASCADE,
    payload_json        TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    expires_at          TEXT NOT NULL
);

CREATE INDEX idx_agent_stream_outbox_expiry
    ON agent_stream_outbox(expires_at, id);
CREATE INDEX idx_agent_stream_outbox_run
    ON agent_stream_outbox(run_id, id) WHERE run_id IS NOT NULL;

-- Migration 46: OTLP traces.
CREATE TABLE otel_spans (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    trace_id            TEXT NOT NULL CHECK (length(trace_id) = 32),
    span_id             TEXT NOT NULL CHECK (length(span_id) = 16),
    parent_span_id      TEXT CHECK (parent_span_id IS NULL OR length(parent_span_id) = 16),
    trace_state         TEXT,
    flags               INTEGER NOT NULL DEFAULT 0,
    span_name           TEXT NOT NULL,
    span_kind           INTEGER NOT NULL,
    start_time_unix_nano INTEGER NOT NULL,
    end_time_unix_nano  INTEGER NOT NULL,
    duration_nano       INTEGER NOT NULL CHECK (duration_nano >= 0),
    status_code         INTEGER NOT NULL DEFAULT 0,
    status_message      TEXT,
    hostname            TEXT NOT NULL DEFAULT '',
    service_name        TEXT,
    service_version     TEXT,
    scope_name          TEXT,
    scope_version       TEXT,
    ai_tool             TEXT,
    ai_project          TEXT,
    ai_session_id       TEXT,
    run_id              INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
    resource_json       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(resource_json)),
    attributes_json     TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes_json)),
    events_json         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(events_json)),
    links_json          TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(links_json)),
    received_at         TEXT NOT NULL,
    content_scrubbed    INTEGER NOT NULL DEFAULT 1 CHECK (content_scrubbed IN (0, 1)),
    UNIQUE(trace_id, span_id)
);

CREATE INDEX idx_otel_spans_run_time
    ON otel_spans(run_id, start_time_unix_nano DESC, id DESC);
CREATE INDEX idx_otel_spans_session_time
    ON otel_spans(hostname, ai_tool, ai_session_id, start_time_unix_nano DESC);
CREATE INDEX idx_otel_spans_trace
    ON otel_spans(trace_id, start_time_unix_nano, span_id);
CREATE INDEX idx_otel_spans_service_time
    ON otel_spans(service_name, start_time_unix_nano DESC);

-- Migration 47: OTLP metric points.
CREATE TABLE otel_metric_points (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    point_key               TEXT NOT NULL UNIQUE,
    metric_name             TEXT NOT NULL,
    description             TEXT NOT NULL DEFAULT '',
    unit                    TEXT NOT NULL DEFAULT '',
    instrument_kind         TEXT NOT NULL CHECK (instrument_kind IN (
        'gauge', 'sum', 'histogram', 'exponential_histogram', 'summary'
    )),
    aggregation_temporality INTEGER,
    monotonic               INTEGER CHECK (monotonic IS NULL OR monotonic IN (0, 1)),
    start_time_unix_nano    INTEGER,
    time_unix_nano          INTEGER NOT NULL,
    hostname                TEXT NOT NULL DEFAULT '',
    service_name            TEXT,
    service_version         TEXT,
    scope_name              TEXT,
    scope_version           TEXT,
    ai_tool                 TEXT,
    ai_project              TEXT,
    ai_session_id           TEXT,
    run_id                  INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
    resource_json           TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(resource_json)),
    attributes_json         TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes_json)),
    value_json              TEXT NOT NULL CHECK (json_valid(value_json)),
    exemplars_json          TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(exemplars_json)),
    received_at             TEXT NOT NULL,
    content_scrubbed        INTEGER NOT NULL DEFAULT 1 CHECK (content_scrubbed IN (0, 1))
);

CREATE INDEX idx_otel_metric_points_run_time
    ON otel_metric_points(run_id, time_unix_nano DESC, id DESC);
CREATE INDEX idx_otel_metric_points_name_time
    ON otel_metric_points(metric_name, time_unix_nano DESC, id DESC);
CREATE INDEX idx_otel_metric_points_session_time
    ON otel_metric_points(hostname, ai_tool, ai_session_id, time_unix_nano DESC);

-- Contract seed rows for projector cursors. Production migration uses INSERT OR IGNORE.
INSERT OR IGNORE INTO agent_projection_cursors(source_name) VALUES
    ('logs'),
    ('mcp_events'),
    ('hook_events'),
    ('skill_events'),
    ('llm_invocations'),
    ('otel_spans'),
    ('otel_metric_points'),
    ('repository_observations');
