# Docker Socket Proxy Ingest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add optional Docker log ingestion from remote Docker hosts through read-only Tecnativa docker-socket-proxy endpoints while preserving normal `docker compose logs -f` behavior on the remote hosts.

**Architecture:** Keep the existing UDP/TCP syslog receiver unchanged and add a second ingest producer that sends `db::LogBatchEntry` values into the same batch writer. Each configured Docker host gets a supervisor task that lists running containers, tails container logs with Docker timestamps, follows Docker events, and persists per-container checkpoints so short outages replay from local Docker logs. Docker socket access remains read-only through docker-socket-proxy.

**Tech Stack:** Rust 1.86, Tokio, SQLite/rusqlite, existing syslog-mcp batch writer, Docker Engine HTTP API through `bollard`, `futures-util` stream helpers, Tecnativa docker-socket-proxy.

---

## File Structure

- Modify `Cargo.toml`: add `bollard` and `futures-util` dependencies.
- Modify `src/lib.rs`: expose a new crate-private `docker_ingest` module.
- Modify `src/config.rs`: add `DockerIngestConfig` and `DockerHostConfig`, TOML/env parsing, and validation.
- Modify `src/config_tests.rs`: cover Docker ingest defaults, TOML host parsing, env host-file parsing, and validation errors.
- Modify `src/runtime.rs`: create one shared ingest channel, start syslog listeners and Docker ingest producers against the shared writer, and abort Docker tasks on runtime drop.
- Modify `src/syslog.rs`: split writer startup from listener startup so non-syslog producers can share the writer.
- Create `src/ingest.rs`: own the shared mpsc channel and writer startup.
- Create `src/docker_ingest.rs`: module facade.
- Create `src/docker_ingest/models.rs`: Docker host/container metadata and checkpoint structs.
- Create `src/docker_ingest/parser.rs`: convert Docker log frames into `db::LogBatchEntry`.
- Create `src/docker_ingest/client.rs`: thin wrapper around `bollard::Docker`.
- Create `src/docker_ingest/supervisor.rs`: host-level task orchestration, reconnect/backoff, container tracking, and event handling.
- Create `src/docker_ingest/checkpoint.rs`: SQLite checkpoint reads/writes.
- Create `src/docker_ingest/*_tests.rs`: sidecar unit tests for parser, config, checkpoints, and supervisor helpers.
- Modify `src/db/pool.rs`: migration 2 creates `docker_ingest_checkpoints`.
- Modify `docs/CONFIG.md`, `docs/mcp/ENV.md`, `.env.example`, `README.md`, and `docs/SETUP.md`: document Docker ingest configuration and socket-proxy requirements.
- Modify `CHANGELOG.md`: add an Unreleased entry for Docker socket-proxy ingest.

## External API Facts

- Use `GET /containers/json?all=true` to discover containers.
- Use `GET /containers/{id}/logs?stdout=true&stderr=true&timestamps=true&follow=true&since=<unix>&tail=all` to replay and follow logs.
- Use `GET /events?filters={"type":["container"]}` to discover start/die/destroy/rename events.
- Docker logs endpoint works for `json-file` and `journald` logging drivers; do not require switching hosts to Docker's `syslog` logging driver.
- Minimum socket-proxy permissions for this feature: `CONTAINERS=1`, `EVENTS=1`, `PING=1`, `VERSION=1`, `POST=0`.

## Task 1: Add Dependencies

**Files:**
- Modify: `Cargo.toml`
- Verify: `Cargo.lock`

- [ ] **Step 1: Add Docker client and stream helpers**

Edit `Cargo.toml` dependencies:

```toml
bollard = { version = "0.19", default-features = false, features = ["http", "chrono"] }
futures-util = "0.3"
```

Keep existing `futures-core = "0.3"` because the MCP SSE code already depends on it.

- [ ] **Step 2: Verify dependency resolution**

Run:

```bash
cargo check
```

Expected: dependency resolution succeeds. If `bollard = "0.19"` is not available in this toolchain's registry snapshot, use the newest available `bollard` version that supports Rust 1.86 and exposes `Docker::connect_with_http`, `Docker::list_containers`, `Docker::logs`, and `Docker::events`.

- [ ] **Step 3: Commit dependency update**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat: add docker ingest dependencies"
```

## Task 2: Add Docker Ingest Configuration

**Files:**
- Modify: `src/config.rs`
- Modify: `src/config_tests.rs`
- Modify: `.env.example`

- [ ] **Step 1: Write failing config tests**

Append tests to `src/config_tests.rs`:

```rust
#[test]
fn docker_ingest_defaults_disabled() {
    let config = Config::default();
    assert!(!config.docker_ingest.enabled);
    assert!(config.docker_ingest.hosts.is_empty());
    assert_eq!(config.docker_ingest.reconnect_initial_ms, 1_000);
    assert_eq!(config.docker_ingest.reconnect_max_ms, 30_000);
    assert_eq!(config.docker_ingest.checkpoint_interval_ms, 5_000);
}

#[test]
fn docker_ingest_toml_hosts_parse() {
    let raw = r#"
        [docker_ingest]
        enabled = true
        reconnect_initial_ms = 250
        reconnect_max_ms = 10000
        checkpoint_interval_ms = 1000

        [[docker_ingest.hosts]]
        name = "nashost"
        base_url = "http://nashost:2375"

        [[docker_ingest.hosts]]
        name = "edgehost"
        base_url = "http://edgehost:2375"
    "#;

    let config: Config = toml::from_str(raw).unwrap();
    assert!(config.docker_ingest.enabled);
    assert_eq!(config.docker_ingest.hosts.len(), 2);
    assert_eq!(config.docker_ingest.hosts[0].name, "nashost");
    assert_eq!(config.docker_ingest.hosts[0].base_url, "http://nashost:2375");
    assert_eq!(config.docker_ingest.hosts[1].name, "edgehost");
    assert_eq!(config.docker_ingest.hosts[1].base_url, "http://edgehost:2375");
}

#[test]
fn docker_ingest_requires_hosts_when_enabled() {
    let mut config = Config::default();
    config.docker_ingest.enabled = true;
    let err = validate_docker_ingest_config(&config.docker_ingest).unwrap_err();
    assert!(err.to_string().contains("docker_ingest.hosts must not be empty"));
}

#[test]
fn docker_ingest_rejects_duplicate_host_names() {
    let mut config = Config::default();
    config.docker_ingest.enabled = true;
    config.docker_ingest.hosts = vec![
        DockerHostConfig {
            name: "nashost".into(),
            base_url: "http://nashost:2375".into(),
        },
        DockerHostConfig {
            name: "nashost".into(),
            base_url: "http://10.0.0.10:2375".into(),
        },
    ];
    let err = validate_docker_ingest_config(&config.docker_ingest).unwrap_err();
    assert!(err.to_string().contains("duplicate docker_ingest host name"));
}
```

- [ ] **Step 2: Run config tests and confirm failure**

Run:

```bash
cargo test config_tests::docker_ingest -- --nocapture
```

Expected: compile failure because `docker_ingest`, `DockerHostConfig`, and `validate_docker_ingest_config` are not defined.

- [ ] **Step 3: Add config structs**

In `src/config.rs`, add `docker_ingest` to `Config`:

```rust
pub struct Config {
    pub syslog: SyslogConfig,
    pub storage: StorageConfig,
    pub mcp: McpConfig,
    pub api: ApiConfig,
    pub docker_ingest: DockerIngestConfig,
}
```

Add structs near `ApiConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DockerIngestConfig {
    pub enabled: bool,
    pub hosts: Vec<DockerHostConfig>,
    pub reconnect_initial_ms: u64,
    pub reconnect_max_ms: u64,
    pub checkpoint_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DockerHostConfig {
    pub name: String,
    pub base_url: String,
}
```

Add defaults:

```rust
impl Default for DockerIngestConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hosts: Vec::new(),
            reconnect_initial_ms: 1_000,
            reconnect_max_ms: 30_000,
            checkpoint_interval_ms: 5_000,
        }
    }
}
```

- [ ] **Step 4: Add env parsing**

In `Config::load()`, after API env parsing:

```rust
env_override_bool("SYSLOG_DOCKER_INGEST_ENABLED", &mut config.docker_ingest.enabled)?;
env_override_parse(
    "SYSLOG_DOCKER_RECONNECT_INITIAL_MS",
    &mut config.docker_ingest.reconnect_initial_ms,
)?;
env_override_parse(
    "SYSLOG_DOCKER_RECONNECT_MAX_MS",
    &mut config.docker_ingest.reconnect_max_ms,
)?;
env_override_parse(
    "SYSLOG_DOCKER_CHECKPOINT_INTERVAL_MS",
    &mut config.docker_ingest.checkpoint_interval_ms,
)?;
if let Ok(path) = std::env::var("SYSLOG_DOCKER_HOSTS_FILE") {
    if !path.is_empty() {
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read SYSLOG_DOCKER_HOSTS_FILE={path}: {e}"))?;
        let parsed: DockerHostsFile = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("Failed to parse SYSLOG_DOCKER_HOSTS_FILE={path}: {e}"))?;
        config.docker_ingest.hosts = parsed.hosts;
    }
}
```

Add helper struct:

```rust
#[derive(Debug, Deserialize)]
struct DockerHostsFile {
    hosts: Vec<DockerHostConfig>,
}
```

- [ ] **Step 5: Add validation**

Call `validate_docker_ingest_config(&config.docker_ingest)?;` in `Config::load()` before `Ok(config)`.

Add:

```rust
fn validate_docker_ingest_config(config: &DockerIngestConfig) -> anyhow::Result<()> {
    if !config.enabled {
        return Ok(());
    }
    if config.hosts.is_empty() {
        return Err(anyhow::anyhow!("docker_ingest.hosts must not be empty when docker ingest is enabled"));
    }
    if config.reconnect_initial_ms == 0 {
        return Err(anyhow::anyhow!("docker_ingest.reconnect_initial_ms must be > 0"));
    }
    if config.reconnect_max_ms < config.reconnect_initial_ms {
        return Err(anyhow::anyhow!("docker_ingest.reconnect_max_ms must be >= reconnect_initial_ms"));
    }
    if config.checkpoint_interval_ms == 0 {
        return Err(anyhow::anyhow!("docker_ingest.checkpoint_interval_ms must be > 0"));
    }

    let mut names = std::collections::HashSet::new();
    for host in &config.hosts {
        if host.name.trim().is_empty() {
            return Err(anyhow::anyhow!("docker_ingest host name must not be empty"));
        }
        if !names.insert(host.name.as_str()) {
            return Err(anyhow::anyhow!("duplicate docker_ingest host name: {}", host.name));
        }
        if !(host.base_url.starts_with("http://") || host.base_url.starts_with("https://")) {
            return Err(anyhow::anyhow!(
                "docker_ingest host {} base_url must start with http:// or https://",
                host.name
            ));
        }
    }
    Ok(())
}
```

- [ ] **Step 6: Update `.env.example`**

Add:

```dotenv
# --- Docker socket-proxy ingest ---

# Keep Docker's normal local/json-file logging driver and have syslog-mcp pull
# container logs through read-only docker-socket-proxy endpoints.
SYSLOG_DOCKER_INGEST_ENABLED=false
# SYSLOG_DOCKER_HOSTS_FILE=/config/docker-hosts.toml
# SYSLOG_DOCKER_RECONNECT_INITIAL_MS=1000
# SYSLOG_DOCKER_RECONNECT_MAX_MS=30000
# SYSLOG_DOCKER_CHECKPOINT_INTERVAL_MS=5000
```

- [ ] **Step 7: Run config tests**

Run:

```bash
cargo test config_tests::docker_ingest -- --nocapture
```

Expected: all Docker ingest config tests pass.

- [ ] **Step 8: Commit config**

```bash
git add src/config.rs src/config_tests.rs .env.example
git commit -m "feat: configure docker socket ingest"
```

## Task 3: Create Shared Ingest Writer Module

**Files:**
- Create: `src/ingest.rs`
- Modify: `src/lib.rs`
- Modify: `src/syslog.rs`
- Test: existing `src/syslog/writer_tests.rs`

- [ ] **Step 1: Create `src/ingest.rs`**

Add:

```rust
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::config::{StorageConfig, SyslogConfig};
use crate::db::{self, DbPool};
use crate::syslog;

pub const WRITE_CHANNEL_CAPACITY: usize = 10_000;

#[derive(Clone)]
pub(crate) struct IngestTx {
    tx: mpsc::Sender<db::LogBatchEntry>,
}

impl IngestTx {
    pub(crate) async fn send(&self, entry: db::LogBatchEntry) -> Result<(), mpsc::error::SendError<db::LogBatchEntry>> {
        self.tx.send(entry).await
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<db::LogBatchEntry> {
        self.tx.clone()
    }
}

pub(crate) fn start_writer(
    storage: StorageConfig,
    pool: Arc<DbPool>,
    storage_state: Arc<Mutex<Option<db::StorageBudgetState>>>,
    batch_size: usize,
    flush_interval_ms: u64,
) -> IngestTx {
    let (tx, rx) = mpsc::channel::<db::LogBatchEntry>(WRITE_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        syslog::writer::batch_writer(
            rx,
            pool,
            storage,
            storage_state,
            batch_size,
            tokio::time::Duration::from_millis(flush_interval_ms),
        )
        .await;
    });
    IngestTx { tx }
}

pub(crate) fn start_writer_from_syslog_config(
    syslog: &SyslogConfig,
    storage: StorageConfig,
    pool: Arc<DbPool>,
    storage_state: Arc<Mutex<Option<db::StorageBudgetState>>>,
) -> IngestTx {
    start_writer(
        storage,
        pool,
        storage_state,
        syslog.batch_size,
        syslog.flush_interval,
    )
}
```

- [ ] **Step 2: Expose modules crate-private**

In `src/lib.rs` add:

```rust
pub(crate) mod ingest;
```

In `src/syslog.rs`, change:

```rust
mod writer;
```

to:

```rust
pub(crate) mod writer;
```

- [ ] **Step 3: Refactor `src/syslog.rs`**

Replace writer creation in `start_with_storage_state()` with a call to `ingest::start_writer_from_syslog_config()`, then pass `ingest_tx.sender()` to UDP and TCP listeners.

Add a second function:

```rust
pub async fn start_listeners(
    config: SyslogConfig,
    tx: tokio::sync::mpsc::Sender<db::LogBatchEntry>,
) -> Result<()> {
    let bind_addr = config.bind_addr();
    let udp_tx = tx.clone();
    let udp_bind = bind_addr.clone();
    let max_size = config.max_message_size;
    tokio::spawn(async move {
        if let Err(e) = listener::udp_listener(&udp_bind, max_size, udp_tx).await {
            error!(error = %e, "UDP syslog listener failed");
        }
    });

    let tcp_tx = tx;
    let tcp_bind = bind_addr.clone();
    let max_tcp_connections = config.max_tcp_connections;
    let tcp_idle_timeout_secs = config.tcp_idle_timeout_secs;
    tokio::spawn(async move {
        if let Err(e) = listener::tcp_listener(
            &tcp_bind,
            tcp_tx,
            max_size,
            max_tcp_connections,
            tcp_idle_timeout_secs,
        )
        .await
        {
            error!(error = %e, "TCP syslog listener failed");
        }
    });

    info!(
        bind = %bind_addr,
        max_message_size = config.max_message_size,
        max_tcp_connections = config.max_tcp_connections,
        tcp_idle_timeout_secs = config.tcp_idle_timeout_secs,
        write_channel_capacity = crate::ingest::WRITE_CHANNEL_CAPACITY,
        "Syslog listeners started"
    );

    Ok(())
}
```

Keep `start_with_storage_state()` as a compatibility wrapper that starts the writer and calls `start_listeners()`.

- [ ] **Step 4: Run existing writer/syslog tests**

Run:

```bash
cargo test syslog:: -- --nocapture
```

Expected: all existing syslog tests pass.

- [ ] **Step 5: Commit shared writer refactor**

```bash
git add src/lib.rs src/ingest.rs src/syslog.rs
git commit -m "refactor: share ingest writer"
```

## Task 4: Add Docker Checkpoint Table

**Files:**
- Modify: `src/db/pool.rs`
- Create: `src/docker_ingest.rs`
- Create: `src/docker_ingest/checkpoint.rs`
- Create: `src/docker_ingest/checkpoint_tests.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add module skeleton**

Create `src/docker_ingest.rs`:

```rust
mod checkpoint;
mod client;
mod models;
mod parser;
mod supervisor;

pub(crate) use supervisor::spawn_all;
```

Create empty module files:

```rust
// src/docker_ingest/client.rs
```

```rust
// src/docker_ingest/models.rs
```

```rust
// src/docker_ingest/parser.rs
```

```rust
// src/docker_ingest/supervisor.rs
```

Add to `src/lib.rs`:

```rust
pub(crate) mod docker_ingest;
```

- [ ] **Step 2: Write checkpoint tests**

Create `src/docker_ingest/checkpoint_tests.rs`:

```rust
use std::sync::Arc;

use crate::config::StorageConfig;
use crate::db;

use super::*;

fn test_pool() -> Arc<db::DbPool> {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("docker-checkpoint.db"));
    Arc::new(db::init_pool(&storage).unwrap())
}

#[test]
fn checkpoint_round_trip() {
    let pool = test_pool();
    save_checkpoint(&pool, "nashost", "abc123", "2026-05-05T01:02:03.456789Z").unwrap();
    let loaded = load_checkpoint(&pool, "nashost", "abc123").unwrap();
    assert_eq!(loaded.as_deref(), Some("2026-05-05T01:02:03.456789Z"));
}

#[test]
fn checkpoint_is_scoped_by_host_and_container() {
    let pool = test_pool();
    save_checkpoint(&pool, "nashost", "abc123", "2026-05-05T01:02:03Z").unwrap();
    assert_eq!(load_checkpoint(&pool, "edgehost", "abc123").unwrap(), None);
    assert_eq!(load_checkpoint(&pool, "nashost", "def456").unwrap(), None);
}
```

- [ ] **Step 3: Run checkpoint tests and confirm failure**

Run:

```bash
cargo test docker_ingest::checkpoint_tests -- --nocapture
```

Expected: compile failure because checkpoint functions and migration do not exist.

- [ ] **Step 4: Add migration 2**

In `src/db/pool.rs`, after migration 1, add:

```rust
let migration_2_applied: bool = conn
    .query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE version = 2",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
    > 0;
if !migration_2_applied {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS docker_ingest_checkpoints (
             host_name      TEXT NOT NULL,
             container_id   TEXT NOT NULL,
             last_timestamp TEXT NOT NULL,
             updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
             PRIMARY KEY (host_name, container_id)
         );
         INSERT INTO schema_migrations (version) VALUES (2);",
    )?;
    tracing::info!("Migration 2: created docker_ingest_checkpoints table");
}
```

- [ ] **Step 5: Implement checkpoint helpers**

Create `src/docker_ingest/checkpoint.rs`:

```rust
use std::sync::Arc;

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use crate::db::DbPool;

pub(super) fn load_checkpoint(
    pool: &Arc<DbPool>,
    host_name: &str,
    container_id: &str,
) -> Result<Option<String>> {
    let conn = pool.get()?;
    let value = conn
        .query_row(
            "SELECT last_timestamp
             FROM docker_ingest_checkpoints
             WHERE host_name = ?1 AND container_id = ?2",
            params![host_name, container_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(value)
}

pub(super) fn save_checkpoint(
    pool: &Arc<DbPool>,
    host_name: &str,
    container_id: &str,
    last_timestamp: &str,
) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO docker_ingest_checkpoints (host_name, container_id, last_timestamp)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(host_name, container_id) DO UPDATE SET
             last_timestamp = excluded.last_timestamp,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![host_name, container_id, last_timestamp],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
```

- [ ] **Step 6: Run checkpoint tests**

Run:

```bash
cargo test docker_ingest::checkpoint_tests -- --nocapture
```

Expected: both checkpoint tests pass.

- [ ] **Step 7: Commit checkpoint migration**

```bash
git add src/lib.rs src/db/pool.rs src/docker_ingest.rs src/docker_ingest/checkpoint.rs src/docker_ingest/checkpoint_tests.rs src/docker_ingest/client.rs src/docker_ingest/models.rs src/docker_ingest/parser.rs src/docker_ingest/supervisor.rs
git commit -m "feat: add docker ingest checkpoints"
```

## Task 5: Parse Docker Log Frames

**Files:**
- Modify: `src/docker_ingest/models.rs`
- Modify: `src/docker_ingest/parser.rs`
- Create: `src/docker_ingest/parser_tests.rs`

- [ ] **Step 1: Add parser tests**

Create `src/docker_ingest/parser_tests.rs`:

```rust
use bollard::container::LogOutput;

use super::*;
use crate::docker_ingest::models::ContainerMeta;

fn meta() -> ContainerMeta {
    ContainerMeta {
        id: "abcdef1234567890".into(),
        name: "nginx-1".into(),
        image: "nginx:latest".into(),
        compose_project: Some("edge".into()),
        compose_service: Some("nginx".into()),
    }
}

#[test]
fn stdout_frame_maps_to_info_log_entry() {
    let entry = log_output_to_entry(
        "nashost",
        &meta(),
        LogOutput::StdOut {
            message: b"2026-05-05T01:02:03.123456789Z started nginx\n".to_vec().into(),
        },
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.timestamp, "2026-05-05T01:02:03.123456789Z");
    assert_eq!(entry.hostname, "nashost");
    assert_eq!(entry.facility.as_deref(), Some("local0"));
    assert_eq!(entry.severity, "info");
    assert_eq!(entry.app_name.as_deref(), Some("edge/nginx/nginx-1"));
    assert_eq!(entry.process_id.as_deref(), Some("abcdef123456"));
    assert_eq!(entry.message, "started nginx");
    assert_eq!(entry.source_ip, "docker://nashost/abcdef1234567890/stdout");
}

#[test]
fn stderr_frame_maps_to_warning_log_entry() {
    let entry = log_output_to_entry(
        "edgehost",
        &meta(),
        LogOutput::StdErr {
            message: b"2026-05-05T01:02:04Z failed health check\n".to_vec().into(),
        },
    )
    .unwrap()
    .unwrap();

    assert_eq!(entry.severity, "warning");
    assert_eq!(entry.message, "failed health check");
    assert_eq!(entry.source_ip, "docker://edgehost/abcdef1234567890/stderr");
}

#[test]
fn non_output_frames_are_ignored() {
    let entry = log_output_to_entry(
        "nashost",
        &meta(),
        LogOutput::Console {
            message: b"ignored\n".to_vec().into(),
        },
    )
    .unwrap();
    assert!(entry.is_none());
}
```

- [ ] **Step 2: Run parser tests and confirm failure**

Run:

```bash
cargo test docker_ingest::parser_tests -- --nocapture
```

Expected: compile failure because parser/model functions do not exist.

- [ ] **Step 3: Add container metadata model**

In `src/docker_ingest/models.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContainerMeta {
    pub id: String,
    pub name: String,
    pub image: String,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
}

impl ContainerMeta {
    pub(super) fn app_name(&self) -> String {
        match (&self.compose_project, &self.compose_service) {
            (Some(project), Some(service)) => format!("{project}/{service}/{}", self.name),
            (_, Some(service)) => format!("{service}/{}", self.name),
            _ => self.name.clone(),
        }
    }

    pub(super) fn short_id(&self) -> String {
        self.id.chars().take(12).collect()
    }
}
```

- [ ] **Step 4: Implement parser**

In `src/docker_ingest/parser.rs`:

```rust
use anyhow::Result;
use bollard::container::LogOutput;

use crate::db;

use super::models::ContainerMeta;

pub(super) fn log_output_to_entry(
    host_name: &str,
    container: &ContainerMeta,
    output: LogOutput,
) -> Result<Option<db::LogBatchEntry>> {
    let (stream, severity, bytes) = match output {
        LogOutput::StdOut { message } => ("stdout", "info", message),
        LogOutput::StdErr { message } => ("stderr", "warning", message),
        _ => return Ok(None),
    };
    let raw_line = String::from_utf8_lossy(&bytes).trim_end_matches(['\r', '\n']).to_string();
    if raw_line.is_empty() {
        return Ok(None);
    }
    let (timestamp, message) = split_docker_timestamp(&raw_line);
    Ok(Some(db::LogBatchEntry {
        timestamp: timestamp.to_string(),
        hostname: host_name.to_string(),
        facility: Some("local0".to_string()),
        severity: severity.to_string(),
        app_name: Some(container.app_name()),
        process_id: Some(container.short_id()),
        message: message.to_string(),
        raw: raw_line,
        source_ip: format!("docker://{}/{}/{}", host_name, container.id, stream),
    }))
}

fn split_docker_timestamp(raw: &str) -> (&str, &str) {
    match raw.split_once(' ') {
        Some((ts, rest)) if chrono::DateTime::parse_from_rfc3339(ts).is_ok() => (ts, rest),
        _ => (chrono::Utc::now().to_rfc3339().leak(), raw),
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
```

During implementation, replace the `leak()` fallback with a small owned helper to avoid leaking memory:

```rust
fn split_docker_timestamp_owned(raw: &str) -> (String, String) {
    match raw.split_once(' ') {
        Some((ts, rest)) if chrono::DateTime::parse_from_rfc3339(ts).is_ok() => {
            (ts.to_string(), rest.to_string())
        }
        _ => (chrono::Utc::now().to_rfc3339(), raw.to_string()),
    }
}
```

Use the owned helper in `log_output_to_entry()`.

- [ ] **Step 5: Run parser tests**

Run:

```bash
cargo test docker_ingest::parser_tests -- --nocapture
```

Expected: all parser tests pass.

- [ ] **Step 6: Commit parser**

```bash
git add src/docker_ingest/models.rs src/docker_ingest/parser.rs src/docker_ingest/parser_tests.rs
git commit -m "feat: map docker logs to log entries"
```

## Task 6: Implement Docker Client Wrapper

**Files:**
- Modify: `src/docker_ingest/client.rs`
- Modify: `src/docker_ingest/models.rs`

- [ ] **Step 1: Implement client wrapper**

In `src/docker_ingest/client.rs`:

```rust
use std::collections::HashMap;

use anyhow::Result;
use bollard::query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder, LogsOptionsBuilder};
use bollard::Docker;

use super::models::ContainerMeta;

#[derive(Clone)]
pub(super) struct DockerHostClient {
    docker: Docker,
}

impl DockerHostClient {
    pub(super) fn connect(base_url: &str) -> Result<Self> {
        let docker = Docker::connect_with_http(base_url, 120, bollard::API_DEFAULT_VERSION)?;
        Ok(Self { docker })
    }

    pub(super) async fn list_containers(&self) -> Result<Vec<ContainerMeta>> {
        let options = ListContainersOptionsBuilder::default().all(true).build();
        let containers = self.docker.list_containers(Some(options)).await?;
        Ok(containers
            .into_iter()
            .filter_map(ContainerMeta::from_summary)
            .collect())
    }

    pub(super) fn logs_options(since_unix: i64) -> bollard::query_parameters::LogsOptions {
        LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .timestamps(true)
            .follow(true)
            .since(since_unix)
            .tail("all")
            .build()
    }

    pub(super) fn container_events_options() -> bollard::query_parameters::EventsOptions {
        let mut filters: HashMap<String, Vec<String>> = HashMap::new();
        filters.insert("type".to_string(), vec!["container".to_string()]);
        EventsOptionsBuilder::default().filters(&filters).build()
    }

    pub(super) fn docker(&self) -> Docker {
        self.docker.clone()
    }
}
```

- [ ] **Step 2: Add `ContainerMeta::from_summary`**

In `src/docker_ingest/models.rs`:

```rust
impl ContainerMeta {
    pub(super) fn from_summary(summary: bollard::models::ContainerSummary) -> Option<Self> {
        let id = summary.id?;
        let name = summary
            .names
            .and_then(|names| names.into_iter().next())
            .map(|name| name.trim_start_matches('/').to_string())
            .unwrap_or_else(|| id.chars().take(12).collect());
        let labels = summary.labels.unwrap_or_default();
        Some(Self {
            id,
            name,
            image: summary.image.unwrap_or_default(),
            compose_project: labels.get("com.docker.compose.project").cloned(),
            compose_service: labels.get("com.docker.compose.service").cloned(),
        })
    }
}
```

- [ ] **Step 3: Run compile check**

Run:

```bash
cargo check
```

Expected: compiles. If builder method names differ in the resolved Bollard version, inspect generated docs with `cargo doc -p bollard --no-deps` or `rg "struct LogsOptions" ~/.cargo/registry/src`.

- [ ] **Step 4: Commit client wrapper**

```bash
git add src/docker_ingest/client.rs src/docker_ingest/models.rs
git commit -m "feat: add docker engine client wrapper"
```

## Task 7: Implement Host Supervisor

**Files:**
- Modify: `src/docker_ingest/supervisor.rs`
- Modify: `src/docker_ingest.rs`
- Test: `cargo check`, parser/checkpoint tests

- [ ] **Step 1: Implement `spawn_all`**

In `src/docker_ingest/supervisor.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use crate::config::{DockerHostConfig, DockerIngestConfig};
use crate::db::{self, DbPool};
use crate::ingest::IngestTx;

use super::checkpoint::{load_checkpoint, save_checkpoint};
use super::client::DockerHostClient;
use super::models::ContainerMeta;
use super::parser::log_output_to_entry;

pub(crate) fn spawn_all(
    config: DockerIngestConfig,
    pool: Arc<DbPool>,
    ingest: IngestTx,
) -> Vec<JoinHandle<()>> {
    if !config.enabled {
        return Vec::new();
    }

    config
        .hosts
        .clone()
        .into_iter()
        .map(|host| {
            let config = config.clone();
            let pool = Arc::clone(&pool);
            let ingest = ingest.clone();
            tokio::spawn(async move {
                run_host_forever(config, host, pool, ingest).await;
            })
        })
        .collect()
}
```

- [ ] **Step 2: Add reconnect loop**

Add:

```rust
async fn run_host_forever(
    config: DockerIngestConfig,
    host: DockerHostConfig,
    pool: Arc<DbPool>,
    ingest: IngestTx,
) {
    let mut delay_ms = config.reconnect_initial_ms;
    loop {
        match run_host_once(&config, &host, Arc::clone(&pool), ingest.clone()).await {
            Ok(()) => {
                tracing::warn!(host = %host.name, "Docker ingest host stream ended; reconnecting");
            }
            Err(e) => {
                tracing::warn!(host = %host.name, error = %e, delay_ms, "Docker ingest host failed; retrying");
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        delay_ms = (delay_ms * 2).min(config.reconnect_max_ms);
    }
}
```

- [ ] **Step 3: Add one-shot host runner**

Add:

```rust
async fn run_host_once(
    config: &DockerIngestConfig,
    host: &DockerHostConfig,
    pool: Arc<DbPool>,
    ingest: IngestTx,
) -> Result<()> {
    let client = DockerHostClient::connect(&host.base_url)?;
    let containers = client.list_containers().await?;
    tracing::info!(
        host = %host.name,
        container_count = containers.len(),
        "Docker ingest discovered containers"
    );

    let mut log_tasks: HashMap<String, JoinHandle<()>> = HashMap::new();
    for container in containers {
        spawn_log_task_if_absent(config, host, &client, Arc::clone(&pool), ingest.clone(), &mut log_tasks, container);
    }

    let docker = client.docker();
    let mut events = docker.events(Some(DockerHostClient::container_events_options()));
    while let Some(event) = events.next().await {
        let event = event?;
        let action = event.action.unwrap_or_default();
        let Some(actor) = event.actor else {
            continue;
        };
        let Some(id) = actor.id else {
            continue;
        };

        match action.as_str() {
            "start" | "restart" | "rename" => {
                let containers = client.list_containers().await?;
                for container in containers.into_iter().filter(|c| c.id == id) {
                    spawn_log_task_if_absent(config, host, &client, Arc::clone(&pool), ingest.clone(), &mut log_tasks, container);
                }
            }
            "die" | "destroy" | "stop" => {
                if let Some(handle) = log_tasks.remove(&id) {
                    handle.abort();
                }
            }
            _ => {}
        }
    }
    Ok(())
}
```

Adjust field names to the resolved Bollard `EventMessage` model if needed.

- [ ] **Step 4: Add log task helper**

Add:

```rust
fn spawn_log_task_if_absent(
    config: &DockerIngestConfig,
    host: &DockerHostConfig,
    client: &DockerHostClient,
    pool: Arc<DbPool>,
    ingest: IngestTx,
    tasks: &mut HashMap<String, JoinHandle<()>>,
    container: ContainerMeta,
) {
    if tasks.contains_key(&container.id) {
        return;
    }
    let docker = client.docker();
    let host_name = host.name.clone();
    let checkpoint_interval = tokio::time::Duration::from_millis(config.checkpoint_interval_ms);
    let container_id = container.id.clone();
    let handle = tokio::spawn(async move {
        let since_unix = load_checkpoint(&pool, &host_name, &container_id)
            .ok()
            .flatten()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(0);
        let mut logs = docker.logs(&container_id, Some(DockerHostClient::logs_options(since_unix)));
        let mut last_checkpoint: Option<String> = None;
        let mut last_flush = tokio::time::Instant::now();

        while let Some(output) = logs.next().await {
            match output {
                Ok(output) => {
                    match log_output_to_entry(&host_name, &container, output) {
                        Ok(Some(entry)) => {
                            last_checkpoint = Some(entry.timestamp.clone());
                            if ingest.send(entry).await.is_err() {
                                tracing::error!(host = %host_name, container_id = %container_id, "Docker ingest channel closed");
                                break;
                            }
                            if last_flush.elapsed() >= checkpoint_interval {
                                if let Some(ts) = &last_checkpoint {
                                    if let Err(e) = save_checkpoint(&pool, &host_name, &container_id, ts) {
                                        tracing::warn!(host = %host_name, container_id = %container_id, error = %e, "Failed to save Docker ingest checkpoint");
                                    }
                                }
                                last_flush = tokio::time::Instant::now();
                            }
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!(host = %host_name, container_id = %container_id, error = %e, "Failed to parse Docker log frame"),
                    }
                }
                Err(e) => {
                    tracing::warn!(host = %host_name, container_id = %container_id, error = %e, "Docker log stream failed");
                    break;
                }
            }
        }

        if let Some(ts) = &last_checkpoint {
            if let Err(e) = save_checkpoint(&pool, &host_name, &container_id, ts) {
                tracing::warn!(host = %host_name, container_id = %container_id, error = %e, "Failed to save final Docker ingest checkpoint");
            }
        }
    });
    tasks.insert(container.id.clone(), handle);
}
```

- [ ] **Step 5: Run compile check**

Run:

```bash
cargo check
```

Expected: compiles. Resolve exact Bollard model field names during this step.

- [ ] **Step 6: Commit supervisor**

```bash
git add src/docker_ingest.rs src/docker_ingest/supervisor.rs
git commit -m "feat: stream docker logs from socket proxy"
```

## Task 8: Wire Docker Ingest Into Runtime

**Files:**
- Modify: `src/runtime.rs`
- Modify: `src/main.rs`
- Test: `src/runtime_tests.rs`

- [ ] **Step 1: Extend runtime handles**

In `src/runtime.rs`, add Docker handles to `MaintenanceHandles` or create a renamed `BackgroundHandles`:

```rust
pub struct MaintenanceHandles {
    purge: Option<JoinHandle<()>>,
    storage: Option<JoinHandle<()>>,
    docker_ingest: Vec<JoinHandle<()>>,
}
```

Update `Drop`:

```rust
for handle in &self.docker_ingest {
    handle.abort();
}
```

- [ ] **Step 2: Start one shared writer in `RuntimeCore`**

Add `ingest: crate::ingest::IngestTx` to `RuntimeCore`.

In `from_config()`, after `service` creation:

```rust
let ingest = crate::ingest::start_writer_from_syslog_config(
    &config.syslog,
    config.storage.clone(),
    Arc::clone(&pool),
    Arc::clone(&storage_state),
);
```

Store it in `Self`.

- [ ] **Step 3: Update syslog startup**

Change `start_syslog()`:

```rust
pub async fn start_syslog(&self) -> Result<()> {
    syslog::start_listeners(self.config.syslog.clone(), self.ingest.sender()).await
}
```

- [ ] **Step 4: Spawn Docker ingest tasks**

In `spawn_maintenance_tasks()`:

```rust
let docker_ingest = crate::docker_ingest::spawn_all(
    self.config.docker_ingest.clone(),
    Arc::clone(&self.pool),
    self.ingest.clone(),
);
MaintenanceHandles { purge, storage, docker_ingest }
```

- [ ] **Step 5: Add startup logging**

In `src/main.rs` startup config log add:

```rust
docker_ingest_enabled = runtime.config.docker_ingest.enabled,
docker_ingest_hosts = runtime.config.docker_ingest.hosts.len(),
```

- [ ] **Step 6: Run runtime tests**

Run:

```bash
cargo test runtime_tests -- --nocapture
```

Expected: runtime tests pass.

- [ ] **Step 7: Run full compile**

Run:

```bash
cargo check
```

Expected: compiles.

- [ ] **Step 8: Commit runtime wiring**

```bash
git add src/runtime.rs src/main.rs
git commit -m "feat: run docker ingest in server runtime"
```

## Task 9: Add Documentation

**Files:**
- Modify: `README.md`
- Modify: `docs/SETUP.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/mcp/ENV.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add config docs**

In `docs/CONFIG.md`, add section:

```markdown
### Docker socket-proxy ingest (`SYSLOG_DOCKER_*`)

Docker ingest is disabled by default. When enabled, syslog-mcp connects to read-only docker-socket-proxy endpoints and stores container stdout/stderr in the same SQLite log table as syslog messages.

| Variable | Required | Default | Sensitive | Description |
| --- | --- | --- | --- | --- |
| `SYSLOG_DOCKER_INGEST_ENABLED` | no | `false` | no | Enable remote Docker log ingestion |
| `SYSLOG_DOCKER_HOSTS_FILE` | yes, when enabled unless TOML hosts are configured | (none) | no | TOML file containing Docker host names and socket-proxy base URLs |
| `SYSLOG_DOCKER_RECONNECT_INITIAL_MS` | no | `1000` | no | Initial reconnect backoff per Docker host |
| `SYSLOG_DOCKER_RECONNECT_MAX_MS` | no | `30000` | no | Maximum reconnect backoff per Docker host |
| `SYSLOG_DOCKER_CHECKPOINT_INTERVAL_MS` | no | `5000` | no | How often to persist per-container replay checkpoints |
```

Add host-file example:

```toml
[[hosts]]
name = "nashost"
base_url = "http://nashost:2375"

[[hosts]]
name = "edgehost"
base_url = "http://edgehost:2375"
```

- [ ] **Step 2: Add setup docs**

In `docs/SETUP.md`, add a Docker host log ingestion section:

```markdown
## Docker host log ingestion

Keep Docker's default `json-file` or `local`/`journald` setup so `docker compose logs -f` remains available on each host. Do not switch Docker daemon logging to the `syslog` driver for this mode.

Each Docker host must expose a read-only docker-socket-proxy reachable by syslog-mcp. Minimum proxy environment:

```dotenv
CONTAINERS=1
EVENTS=1
PING=1
VERSION=1
POST=0
EXEC=0
ALLOW_START=0
ALLOW_STOP=0
ALLOW_RESTARTS=0
```

Verify from the syslog-mcp host:

```bash
curl http://nashost:2375/_ping
curl 'http://nashost:2375/containers/json?all=true'
```
```

- [ ] **Step 3: Add README summary**

In `README.md`, add the feature under the existing syslog forwarder section:

```markdown
### Docker container logs through docker-socket-proxy

syslog-mcp can optionally ingest container stdout/stderr from remote Docker hosts through read-only docker-socket-proxy endpoints. This preserves `docker compose logs -f` because Docker continues using its normal local logging driver. The ingester stores rows with `hostname=<docker host>`, `app_name=<compose project/service/container>`, and `source_ip=docker://<host>/<container>/<stream>`.
```

- [ ] **Step 4: Update `docs/mcp/ENV.md`**

Add the same Docker env variables in concise table form.

- [ ] **Step 5: Update changelog**

Under `## [Unreleased]`, add:

```markdown
### Added

- **Docker ingest**: optional remote Docker container log ingestion through read-only docker-socket-proxy endpoints. Keeps Docker's local logging path intact for `docker compose logs -f` and stores container stdout/stderr in the existing SQLite log table.
```

- [ ] **Step 6: Commit docs**

```bash
git add README.md docs/SETUP.md docs/CONFIG.md docs/mcp/ENV.md CHANGELOG.md .env.example
git commit -m "docs: document docker socket ingest"
```

## Task 10: Integration Verification Against `nashost` and `edgehost`

**Files:**
- Create local untracked config for manual verification: `docker-hosts.local.toml`
- No committed code changes in this task unless verification exposes a defect.

- [ ] **Step 1: Create local host file**

Create `docker-hosts.local.toml`:

```toml
[[hosts]]
name = "nashost"
base_url = "http://nashost:2375"

[[hosts]]
name = "edgehost"
base_url = "http://edgehost:2375"
```

Keep this file untracked.

- [ ] **Step 2: Start server with Docker ingest enabled**

Run:

```bash
SYSLOG_DOCKER_INGEST_ENABLED=true \
SYSLOG_DOCKER_HOSTS_FILE=docker-hosts.local.toml \
SYSLOG_MCP_DB_PATH=/tmp/syslog-mcp-docker-ingest.db \
cargo run
```

Expected logs:

```text
Docker ingest discovered containers
Docker ingest host
MCP server listening
```

- [ ] **Step 3: Query recent Docker logs**

In another shell:

```bash
curl -s -X POST http://localhost:3100/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tail_logs","arguments":{"source_ip":"docker://nashost","n":5}}}' | jq .
```

If `source_ip` exact filtering cannot prefix-match, use:

```bash
sqlite3 /tmp/syslog-mcp-docker-ingest.db \
  "SELECT hostname, app_name, source_ip, substr(message,1,80) FROM logs WHERE source_ip LIKE 'docker://nashost/%' ORDER BY id DESC LIMIT 5;"
```

Expected: rows with `hostname='nashost'` and `source_ip` beginning `docker://nashost/`.

- [ ] **Step 4: Verify replay checkpoint**

Stop the server with Ctrl-C, then run it again with the same DB path and host file.

Run:

```bash
sqlite3 /tmp/syslog-mcp-docker-ingest.db \
  "SELECT host_name, substr(container_id,1,12), last_timestamp FROM docker_ingest_checkpoints ORDER BY host_name, container_id LIMIT 10;"
```

Expected: checkpoints exist for `nashost` and `edgehost` containers.

- [ ] **Step 5: Verify remote containers are not affected**

On each host:

```bash
ssh nashost 'docker compose logs --tail=1 2>/dev/null || docker logs --tail=1 $(docker ps -q | head -n1)'
ssh edgehost 'docker compose logs --tail=1 2>/dev/null || docker logs --tail=1 $(docker ps -q | head -n1)'
```

Expected: Docker local logs still work because daemon logging driver was not changed.

## Task 11: Full Test and Lint

**Files:**
- Entire repo

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --check
```

Expected: passes. If it fails, run `cargo fmt`, inspect the diff, and commit formatting with the relevant code changes.

- [ ] **Step 2: Clippy**

Run:

```bash
cargo clippy -- -D warnings
```

Expected: passes.

- [ ] **Step 3: Full test suite**

Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 4: Version sync check**

Run:

```bash
bash bin/check-version-sync.sh
```

Expected: all version-bearing files match. If the branch is going to be pushed, apply the repo's version bump rule before commit/push.

## Open Risks

- Docker Engine `/containers/{id}/logs` is only supported for containers using `json-file` or `journald`; hosts using only Docker's `local` driver may need a live verification pass. If `local` does not support the API on these hosts, use `journald` or `json-file` for containers that must be centrally ingested.
- `source_ip` filters are exact-match today. Docker rows use `docker://host/container/stream`, so prefix filtering would require a separate future query enhancement if MCP users need `source_ip=docker://nashost` to match all nashost containers.
- `CONTAINERS=1` grants read access to all container endpoints under the proxy's container API section, not only logs. Keep port `2375` firewalled to trusted networks.

## Self-Review

- Spec coverage: the plan covers remote Docker hosts, docker-socket-proxy permissions, Docker API endpoints, stream/replay behavior, syslog-mcp field mapping, runtime wiring, checkpoints, documentation, and verification against `nashost`/`edgehost`.
- Placeholder scan: no task relies on unstated implementation details; each code task names files, concrete commands, and expected results.
- Type consistency: `DockerIngestConfig`, `DockerHostConfig`, `ContainerMeta`, `IngestTx`, and checkpoint function names are introduced before later tasks use them.
