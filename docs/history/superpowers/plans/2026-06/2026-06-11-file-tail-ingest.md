# Managed File-Tail Ingest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Cortex-owned file-tail ingestion so operators can register log files such as SWAG, Authelia, AdGuard, fail2ban, and AI transcripts through CLI, REST API, and MCP without hand-maintained rsyslog `imfile` drop-ins.

**Architecture:** Add a small file-tail subsystem that persists source definitions in `data/file-tails.json`, spawns one supervised Tokio task per enabled source, converts appended lines into `LogBatchEntry` rows, and sends them through the existing ingest writer/enrichment pipeline. Expose one admin MCP action, `file_tails`, with `op=list|add|remove|enable|disable|status`; REST and CLI call the same service methods and request/response models.

**Tech Stack:** Rust 2024, Tokio async file IO, serde JSON registry, existing `IngestTx`, existing `CortexService`, Axum REST, single-action RMCP dispatch, existing hand-rolled CLI parser.

---

## File Structure

- Create `src/file_tail.rs`: module entrypoint and public crate-internal re-exports.
- Create `src/file_tail/models.rs`: persisted source definitions plus shared request/response DTOs.
- Create `src/file_tail/registry.rs`: load/save/update `data/file-tails.json` atomically.
- Create `src/file_tail/supervisor.rs`: runtime tail task management and line-to-`LogBatchEntry` conversion.
- Create `src/file_tail/models_tests.rs`, `registry_tests.rs`, `supervisor_tests.rs`: focused unit tests.
- Modify `src/lib.rs`: add `pub mod file_tail;`.
- Modify `src/enrich/parser.rs`: add `SourceKind::FileTail` with wire value `file-tail`.
- Modify `src/runtime.rs`: create registry/supervisor in `RuntimeCore`, spawn file-tail tasks with maintenance handles, and expose control through `CortexService`.
- Modify `src/app.rs`, `src/app/models.rs`, `src/app/models/ops.rs`, `src/app/services.rs`, and create `src/app/services/file_tails.rs`: service-layer request validation and control methods.
- Modify `src/mcp/actions.rs`, `src/mcp/tools.rs`, `src/mcp/schemas.rs`: add admin MCP action `file_tails`.
- Modify `src/api.rs`, `src/cli/http_client.rs`: add REST route `POST /api/file-tails` and HTTP client method.
- Modify `src/cli/args.rs`, `src/cli/parse.rs`, `src/cli/run.rs`, `src/cli/dispatch.rs`, and create `src/cli/commands/file_tails.rs`: CLI `cortex file-tail ...`.
- Modify docs: `README.md`, `CLAUDE.md`, `docs/CLI.md`, `docs/api.md`, `docs/mcp/SCHEMA.md`, `docs/CONFIG.md`, `docs/contracts/source-kinds.md`, `.env.example`, `config.toml`.
- Version bump files: `Cargo.toml`, `Cargo.lock`, `server.json`, `mcpb/manifest.json`, `CHANGELOG.md`.

---

### Task 1: Add File-Tail Models And SourceKind

**Files:**
- Create: `src/file_tail.rs`
- Create: `src/file_tail/models.rs`
- Test: `src/file_tail/models_tests.rs`
- Modify: `src/lib.rs`
- Modify: `src/enrich/parser.rs`
- Test: `src/enrich/parser_tests.rs`

- [ ] **Step 1: Write failing model and source-kind tests**

Create `src/file_tail/models_tests.rs`:

```rust
use super::models::*;

#[test]
fn add_request_builds_enabled_source_with_defaults() {
    let req = FileTailAddRequest {
        id: "swag-access".into(),
        path: "/mnt/appdata/swag/log/nginx/access.log".into(),
        tag: "swag-access".into(),
        hostname: Some("edgehost".into()),
        facility: None,
        severity: None,
        start_at_end: None,
    };

    let source = FileTailSource::from_add(req, "2026-06-11T20:00:00Z");

    assert_eq!(source.id, "swag-access");
    assert_eq!(source.path, "/mnt/appdata/swag/log/nginx/access.log");
    assert_eq!(source.tag, "swag-access");
    assert_eq!(source.hostname.as_deref(), Some("edgehost"));
    assert_eq!(source.facility.as_deref(), Some("local7"));
    assert_eq!(source.severity, "info");
    assert!(source.start_at_end);
    assert!(source.enabled);
    assert_eq!(source.created_at, "2026-06-11T20:00:00Z");
    assert_eq!(source.updated_at, "2026-06-11T20:00:00Z");
}

#[test]
fn file_tail_request_rejects_missing_fields_for_add() {
    let req = FileTailRequest {
        op: FileTailOp::Add,
        id: None,
        path: None,
        tag: None,
        hostname: None,
        facility: None,
        severity: None,
        start_at_end: None,
    };

    assert_eq!(
        req.validate().unwrap_err(),
        "file_tails op=add requires id, path, and tag"
    );
}

#[test]
fn file_tail_request_rejects_path_traversal_ids() {
    let req = FileTailRequest {
        op: FileTailOp::Remove,
        id: Some("../swag".into()),
        path: None,
        tag: None,
        hostname: None,
        facility: None,
        severity: None,
        start_at_end: None,
    };

    assert_eq!(
        req.validate().unwrap_err(),
        "file_tails id must contain only ASCII letters, digits, dot, underscore, or dash"
    );
}
```

Append to `src/enrich/parser_tests.rs` or create the sidecar test if no direct spot exists:

```rust
#[test]
fn source_kind_file_tail_wire_value_is_stable() {
    assert_eq!(crate::enrich::SourceKind::FileTail.as_str(), "file-tail");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test file_tail --lib
cargo test source_kind_file_tail_wire_value_is_stable --lib
```

Expected: compile fails because `file_tail` module, DTOs, and `SourceKind::FileTail` do not exist.

- [ ] **Step 3: Add the models**

Create `src/file_tail.rs`:

```rust
pub(crate) mod models;
pub(crate) mod registry;
pub(crate) mod supervisor;

pub(crate) use models::{
    FileTailAddRequest, FileTailOp, FileTailRequest, FileTailResponse, FileTailSource,
    FileTailStatus,
};
pub(crate) use registry::FileTailRegistry;
pub(crate) use supervisor::FileTailSupervisor;

#[cfg(test)]
#[path = "file_tail/models_tests.rs"]
mod models_tests;
```

Create `src/file_tail/models.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileTailSource {
    pub id: String,
    pub path: String,
    pub tag: String,
    pub hostname: Option<String>,
    pub facility: Option<String>,
    pub severity: String,
    pub start_at_end: bool,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileTailOp {
    List,
    Add,
    Remove,
    Enable,
    Disable,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileTailRequest {
    pub op: FileTailOp,
    pub id: Option<String>,
    pub path: Option<String>,
    pub tag: Option<String>,
    pub hostname: Option<String>,
    pub facility: Option<String>,
    pub severity: Option<String>,
    pub start_at_end: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileTailAddRequest {
    pub id: String,
    pub path: String,
    pub tag: String,
    pub hostname: Option<String>,
    pub facility: Option<String>,
    pub severity: Option<String>,
    pub start_at_end: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FileTailStatus {
    pub id: String,
    pub running: bool,
    pub last_line_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FileTailResponse {
    pub sources: Vec<FileTailSource>,
    pub statuses: Vec<FileTailStatus>,
}

impl FileTailSource {
    pub(crate) fn from_add(req: FileTailAddRequest, now: &str) -> Self {
        Self {
            id: req.id,
            path: req.path,
            tag: req.tag,
            hostname: req.hostname,
            facility: Some(req.facility.unwrap_or_else(|| "local7".to_string())),
            severity: req.severity.unwrap_or_else(|| "info".to_string()),
            start_at_end: req.start_at_end.unwrap_or(true),
            enabled: true,
            created_at: now.to_string(),
            updated_at: now.to_string(),
        }
    }
}

impl FileTailRequest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        match self.op {
            FileTailOp::List | FileTailOp::Status => Ok(()),
            FileTailOp::Add => {
                let Some(id) = self.id.as_deref() else {
                    return Err("file_tails op=add requires id, path, and tag".into());
                };
                validate_id(id)?;
                if self.path.as_deref().is_none_or(str::is_empty)
                    || self.tag.as_deref().is_none_or(str::is_empty)
                {
                    return Err("file_tails op=add requires id, path, and tag".into());
                }
                Ok(())
            }
            FileTailOp::Remove | FileTailOp::Enable | FileTailOp::Disable => {
                let Some(id) = self.id.as_deref() else {
                    return Err(format!("file_tails op={:?} requires id", self.op).to_lowercase());
                };
                validate_id(id)
            }
        }
    }

    pub(crate) fn into_add(self) -> Result<FileTailAddRequest, String> {
        self.validate()?;
        Ok(FileTailAddRequest {
            id: self.id.expect("validated id"),
            path: self.path.expect("validated path"),
            tag: self.tag.expect("validated tag"),
            hostname: self.hostname,
            facility: self.facility,
            severity: self.severity,
            start_at_end: self.start_at_end,
        })
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(
            "file_tails id must contain only ASCII letters, digits, dot, underscore, or dash".into(),
        );
    }
    Ok(())
}
```

Modify `src/lib.rs`:

```rust
pub mod file_tail;
```

Modify `src/enrich/parser.rs`:

```rust
pub enum SourceKind {
    SyslogUdp,
    SyslogTcp,
    DockerStream,
    DockerEvent,
    Otlp,
    AdguardApi,
    UnifiApi,
    Agent,
    ShellHistory,
    AgentCommand,
    FileTail,
}
```

and in `SourceKind::as_str()`:

```rust
SourceKind::FileTail => "file-tail",
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test file_tail --lib
cargo test source_kind_file_tail_wire_value_is_stable --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/lib.rs src/enrich/parser.rs src/enrich/parser_tests.rs src/file_tail.rs src/file_tail/models.rs src/file_tail/models_tests.rs
git commit -m "feat: add file-tail ingest models"
```

---

### Task 2: Persist File-Tail Sources

**Files:**
- Create: `src/file_tail/registry.rs`
- Test: `src/file_tail/registry_tests.rs`
- Modify: `src/file_tail.rs`

- [ ] **Step 1: Write failing registry tests**

Create `src/file_tail/registry_tests.rs`:

```rust
use super::models::{FileTailAddRequest, FileTailSource};
use super::registry::FileTailRegistry;

#[test]
fn registry_adds_lists_and_removes_sources() {
    let temp = tempfile::tempdir().unwrap();
    let registry = FileTailRegistry::new(temp.path().join("file-tails.json"));
    let source = FileTailSource::from_add(
        FileTailAddRequest {
            id: "swag-access".into(),
            path: "/tmp/access.log".into(),
            tag: "swag-access".into(),
            hostname: Some("edgehost".into()),
            facility: None,
            severity: None,
            start_at_end: None,
        },
        "2026-06-11T20:00:00Z",
    );

    registry.upsert(source.clone()).unwrap();
    assert_eq!(registry.list().unwrap(), vec![source]);

    registry.remove("swag-access").unwrap();
    assert!(registry.list().unwrap().is_empty());
}

#[test]
fn registry_persists_across_instances() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("file-tails.json");
    let registry = FileTailRegistry::new(path.clone());
    registry
        .upsert(FileTailSource::from_add(
            FileTailAddRequest {
                id: "authelia".into(),
                path: "/tmp/authelia.log".into(),
                tag: "authelia".into(),
                hostname: None,
                facility: Some("local5".into()),
                severity: Some("info".into()),
                start_at_end: Some(false),
            },
            "2026-06-11T20:00:00Z",
        ))
        .unwrap();

    let reloaded = FileTailRegistry::new(path);
    let sources = reloaded.list().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].id, "authelia");
    assert_eq!(sources[0].facility.as_deref(), Some("local5"));
    assert!(!sources[0].start_at_end);
}
```

Add to `src/file_tail.rs`:

```rust
#[cfg(test)]
#[path = "file_tail/registry_tests.rs"]
mod registry_tests;
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test registry_ --lib
```

Expected: compile fails because `FileTailRegistry` does not exist.

- [ ] **Step 3: Implement registry**

Create `src/file_tail/registry.rs`:

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use parking_lot::Mutex;

use super::models::FileTailSource;

#[derive(Debug)]
pub(crate) struct FileTailRegistry {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileTailRegistry {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub(crate) fn path_from_storage_db(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("file-tails.json")
    }

    pub(crate) fn list(&self) -> Result<Vec<FileTailSource>> {
        let _guard = self.lock.lock();
        self.read_locked()
    }

    pub(crate) fn upsert(&self, source: FileTailSource) -> Result<()> {
        let _guard = self.lock.lock();
        let mut sources = self.read_locked()?;
        sources.retain(|existing| existing.id != source.id);
        sources.push(source);
        sources.sort_by(|a, b| a.id.cmp(&b.id));
        self.write_locked(&sources)
    }

    pub(crate) fn remove(&self, id: &str) -> Result<()> {
        let _guard = self.lock.lock();
        let mut sources = self.read_locked()?;
        sources.retain(|existing| existing.id != id);
        self.write_locked(&sources)
    }

    pub(crate) fn set_enabled(&self, id: &str, enabled: bool, now: &str) -> Result<()> {
        let _guard = self.lock.lock();
        let mut sources = self.read_locked()?;
        let source = sources
            .iter_mut()
            .find(|source| source.id == id)
            .with_context(|| format!("file tail source not found: {id}"))?;
        source.enabled = enabled;
        source.updated_at = now.to_string();
        self.write_locked(&sources)
    }

    fn read_locked(&self) -> Result<Vec<FileTailSource>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let raw = std::fs::read_to_string(&self.path)
            .with_context(|| format!("read {}", self.path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parse {}", self.path.display()))
    }

    fn write_locked(&self, sources: &[FileTailSource]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(sources)?;
        std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("replace {}", self.path.display()))?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test registry_ --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/file_tail.rs src/file_tail/registry.rs src/file_tail/registry_tests.rs
git commit -m "feat: persist file-tail sources"
```

---

### Task 3: Tail Files Into The Existing Ingest Pipeline

**Files:**
- Create: `src/file_tail/supervisor.rs`
- Test: `src/file_tail/supervisor_tests.rs`
- Modify: `src/file_tail.rs`
- Modify: `src/observability.rs`

- [ ] **Step 1: Write failing supervisor tests**

Create `src/file_tail/supervisor_tests.rs`:

```rust
use tokio::io::AsyncWriteExt;

use crate::db::LogBatchEntry;
use crate::ingest::IngestTx;

use super::models::FileTailSource;
use super::supervisor::{file_tail_line_to_entry, tail_file_once_for_test};

#[test]
fn file_tail_line_to_entry_sets_expected_envelope() {
    let source = FileTailSource {
        id: "swag-access".into(),
        path: "/tmp/access.log".into(),
        tag: "swag-access".into(),
        hostname: Some("edgehost".into()),
        facility: Some("local4".into()),
        severity: "info".into(),
        start_at_end: true,
        enabled: true,
        created_at: "2026-06-11T20:00:00Z".into(),
        updated_at: "2026-06-11T20:00:00Z".into(),
    };

    let entry = file_tail_line_to_entry(&source, "GET / HTTP/1.1\" 401", "2026-06-11T20:01:00Z");

    assert_eq!(entry.timestamp, "2026-06-11T20:01:00Z");
    assert_eq!(entry.hostname, "edgehost");
    assert_eq!(entry.facility.as_deref(), Some("local4"));
    assert_eq!(entry.severity, "info");
    assert_eq!(entry.app_name.as_deref(), Some("swag-access"));
    assert_eq!(entry.message, "GET / HTTP/1.1\" 401");
    assert_eq!(entry.raw, "GET / HTTP/1.1\" 401");
    assert_eq!(entry.source_ip, "file-tail://edgehost/swag-access");
    assert!(entry.metadata_json.as_deref().unwrap().contains("\"source_kind\":\"file-tail\""));
    assert!(entry.metadata_json.as_deref().unwrap().contains("\"path\":\"/tmp/access.log\""));
}

#[tokio::test]
async fn tail_file_once_sends_existing_lines_when_not_starting_at_end() {
    let temp = tempfile::tempdir().unwrap();
    let file_path = temp.path().join("authelia.log");
    let mut file = tokio::fs::File::create(&file_path).await.unwrap();
    file.write_all(b"time=one level=info\n").await.unwrap();
    file.write_all(b"time=two level=error\n").await.unwrap();
    file.flush().await.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<LogBatchEntry>(4);
    let ingest = IngestTx::from_sender_for_test(tx);
    let source = FileTailSource {
        id: "authelia".into(),
        path: file_path.to_string_lossy().into_owned(),
        tag: "authelia".into(),
        hostname: Some("edgehost".into()),
        facility: Some("local5".into()),
        severity: "info".into(),
        start_at_end: false,
        enabled: true,
        created_at: "2026-06-11T20:00:00Z".into(),
        updated_at: "2026-06-11T20:00:00Z".into(),
    };

    tail_file_once_for_test(source, ingest).await.unwrap();

    assert_eq!(rx.recv().await.unwrap().message, "time=one level=info");
    assert_eq!(rx.recv().await.unwrap().message, "time=two level=error");
    assert!(rx.try_recv().is_err());
}
```

Add to `src/file_tail.rs`:

```rust
#[cfg(test)]
#[path = "file_tail/supervisor_tests.rs"]
mod supervisor_tests;
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test file_tail_line_to_entry_sets_expected_envelope tail_file_once_sends_existing_lines_when_not_starting_at_end --lib
```

Expected: compile fails because supervisor helpers do not exist.

- [ ] **Step 3: Implement file-tail conversion and basic tail helper**

Create `src/file_tail/supervisor.rs` with this minimum content:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::LogBatchEntry;
use crate::enrich::{SourceKind, stamp_source_kind};
use crate::ingest::IngestTx;
use crate::ingest_metadata::bounded_metadata_json;

use super::models::{FileTailSource, FileTailStatus};
use super::registry::FileTailRegistry;

#[derive(Clone)]
pub(crate) struct FileTailSupervisor {
    registry: Arc<FileTailRegistry>,
    ingest: IngestTx,
    token: CancellationToken,
    tasks: Arc<Mutex<HashMap<String, TailTask>>>,
}

struct TailTask {
    handle: JoinHandle<()>,
    status: Arc<Mutex<FileTailStatus>>,
}

impl FileTailSupervisor {
    pub(crate) fn new(
        registry: Arc<FileTailRegistry>,
        ingest: IngestTx,
        token: CancellationToken,
    ) -> Self {
        Self {
            registry,
            ingest,
            token,
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn statuses(&self) -> Vec<FileTailStatus> {
        let mut out: Vec<_> = self
            .tasks
            .lock()
            .values()
            .map(|task| task.status.lock().clone())
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub(crate) fn reconcile(&self) -> Result<()> {
        let sources = self.registry.list()?;
        let enabled: std::collections::HashSet<String> = sources
            .iter()
            .filter(|source| source.enabled)
            .map(|source| source.id.clone())
            .collect();

        {
            let mut tasks = self.tasks.lock();
            tasks.retain(|id, task| {
                if enabled.contains(id) {
                    true
                } else {
                    task.handle.abort();
                    false
                }
            });
        }

        for source in sources.into_iter().filter(|source| source.enabled) {
            if self.tasks.lock().contains_key(&source.id) {
                continue;
            }
            self.spawn_source(source);
        }
        Ok(())
    }

    fn spawn_source(&self, source: FileTailSource) {
        let id = source.id.clone();
        let status = Arc::new(Mutex::new(FileTailStatus {
            id: id.clone(),
            running: true,
            last_line_at: None,
            last_error: None,
        }));
        let task_status = Arc::clone(&status);
        let ingest = self.ingest.clone();
        let token = self.token.clone();
        let handle = tokio::spawn(async move {
            tail_file_loop(source, ingest, token, task_status).await;
        });
        self.tasks.lock().insert(id, TailTask { handle, status });
    }
}

async fn tail_file_loop(
    source: FileTailSource,
    ingest: IngestTx,
    token: CancellationToken,
    status: Arc<Mutex<FileTailStatus>>,
) {
    loop {
        if token.is_cancelled() {
            status.lock().running = false;
            return;
        }
        match tail_file_until_cancelled(&source, ingest.clone(), token.clone(), Arc::clone(&status)).await {
            Ok(()) => {
                status.lock().running = false;
                return;
            }
            Err(err) => {
                status.lock().last_error = Some(err.to_string());
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn tail_file_until_cancelled(
    source: &FileTailSource,
    ingest: IngestTx,
    token: CancellationToken,
    status: Arc<Mutex<FileTailStatus>>,
) -> Result<()> {
    let mut file = tokio::fs::File::open(&source.path)
        .await
        .with_context(|| format!("open {}", source.path))?;
    if source.start_at_end {
        file.seek(std::io::SeekFrom::End(0)).await?;
    }
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        tokio::select! {
            _ = token.cancelled() => return Ok(()),
            read = reader.read_line(&mut line) => {
                let bytes = read?;
                if bytes == 0 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                let msg = line.trim_end_matches(['\r', '\n']);
                if msg.is_empty() {
                    continue;
                }
                let now = now_iso();
                let entry = file_tail_line_to_entry(source, msg, &now);
                ingest.send(entry).await?;
                let mut status = status.lock();
                status.last_line_at = Some(now);
                status.last_error = None;
            }
        }
    }
}

pub(crate) fn file_tail_line_to_entry(
    source: &FileTailSource,
    line: &str,
    now: &str,
) -> LogBatchEntry {
    let hostname = source
        .hostname
        .clone()
        .unwrap_or_else(|| local_hostname());
    let metadata_json = bounded_metadata_json(serde_json::json!({
        "source_type": "file_tail",
        "source_kind": SourceKind::FileTail.as_str(),
        "file_tail_id": source.id,
        "path": source.path,
        "tag": source.tag,
    }));
    let mut entry = LogBatchEntry {
        timestamp: now.to_string(),
        hostname: hostname.clone(),
        facility: source.facility.clone(),
        severity: source.severity.clone(),
        app_name: Some(source.tag.clone()),
        process_id: None,
        message: line.to_string(),
        raw: line.to_string(),
        source_ip: format!("file-tail://{hostname}/{}", source.id),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: Some(metadata_json),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    };
    stamp_source_kind(&mut entry, SourceKind::FileTail);
    entry
}

#[cfg(test)]
pub(crate) async fn tail_file_once_for_test(
    source: FileTailSource,
    ingest: IngestTx,
) -> Result<()> {
    let file = tokio::fs::File::open(&source.path).await?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 {
        let msg = line.trim_end_matches(['\r', '\n']);
        if !msg.is_empty() {
            ingest
                .send(file_tail_line_to_entry(&source, msg, "2026-06-11T20:01:00Z"))
                .await?;
        }
        line.clear();
    }
    Ok(())
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn local_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|host| !host.trim().is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test file_tail_line_to_entry_sets_expected_envelope tail_file_once_sends_existing_lines_when_not_starting_at_end --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/file_tail.rs src/file_tail/supervisor.rs src/file_tail/supervisor_tests.rs
git commit -m "feat: tail files into ingest pipeline"
```

---

### Task 4: Wire Runtime And Service Control

**Files:**
- Modify: `src/runtime.rs`
- Modify: `src/app.rs`
- Modify: `src/app/models.rs`
- Modify: `src/app/models/ops.rs`
- Modify: `src/app/services.rs`
- Create: `src/app/services/file_tails.rs`
- Test: `src/app/service_tests.rs`
- Test: `src/runtime_tests.rs`

- [ ] **Step 1: Write failing service tests**

Append to `src/app/service_tests.rs`:

```rust
#[tokio::test]
async fn file_tails_add_list_disable_enable_remove_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let storage = test_storage(temp.path());
    let pool = std::sync::Arc::new(crate::db::init_pool(&storage).unwrap());
    let registry = std::sync::Arc::new(crate::file_tail::FileTailRegistry::new(
        temp.path().join("file-tails.json"),
    ));
    let service = CortexService::new(pool, storage).with_file_tail_registry(registry);

    let add = service
        .file_tails(crate::app::FileTailRequest {
            op: crate::app::FileTailOp::Add,
            id: Some("swag-access".into()),
            path: Some("/tmp/access.log".into()),
            tag: Some("swag-access".into()),
            hostname: Some("edgehost".into()),
            facility: Some("local4".into()),
            severity: Some("info".into()),
            start_at_end: Some(true),
        })
        .await
        .unwrap();
    assert_eq!(add.sources[0].id, "swag-access");

    let disabled = service
        .file_tails(crate::app::FileTailRequest {
            op: crate::app::FileTailOp::Disable,
            id: Some("swag-access".into()),
            path: None,
            tag: None,
            hostname: None,
            facility: None,
            severity: None,
            start_at_end: None,
        })
        .await
        .unwrap();
    assert!(!disabled.sources[0].enabled);

    let enabled = service
        .file_tails(crate::app::FileTailRequest {
            op: crate::app::FileTailOp::Enable,
            id: Some("swag-access".into()),
            path: None,
            tag: None,
            hostname: None,
            facility: None,
            severity: None,
            start_at_end: None,
        })
        .await
        .unwrap();
    assert!(enabled.sources[0].enabled);

    let removed = service
        .file_tails(crate::app::FileTailRequest {
            op: crate::app::FileTailOp::Remove,
            id: Some("swag-access".into()),
            path: None,
            tag: None,
            hostname: None,
            facility: None,
            severity: None,
            start_at_end: None,
        })
        .await
        .unwrap();
    assert!(removed.sources.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test file_tails_add_list_disable_enable_remove_round_trip --lib
```

Expected: compile fails because service methods and app exports do not exist.

- [ ] **Step 3: Re-export file-tail DTOs from app**

Modify `src/app/models/ops.rs`:

```rust
pub use crate::file_tail::{
    FileTailAddRequest, FileTailOp, FileTailRequest, FileTailResponse, FileTailSource,
    FileTailStatus,
};
```

Modify `src/app.rs` export list:

```rust
FileTailAddRequest,
FileTailOp,
FileTailRequest,
FileTailResponse,
FileTailSource,
FileTailStatus,
```

- [ ] **Step 4: Add registry/control to service**

Modify `src/app/services.rs` imports:

```rust
use crate::file_tail::{FileTailRegistry, FileTailRequest, FileTailResponse};
```

Add fields to `CortexService`:

```rust
file_tail_registry: Option<Arc<FileTailRegistry>>,
file_tail_reconcile: Option<Arc<dyn Fn() -> anyhow::Result<()> + Send + Sync>>,
file_tail_statuses: Option<Arc<dyn Fn() -> Vec<crate::file_tail::FileTailStatus> + Send + Sync>>,
```

Initialize them to `None` in both constructors.

Add builder:

```rust
pub(crate) fn with_file_tail_registry(mut self, registry: Arc<FileTailRegistry>) -> Self {
    self.file_tail_registry = Some(registry);
    self
}

pub(crate) fn with_file_tail_control(
    mut self,
    registry: Arc<FileTailRegistry>,
    reconcile: Arc<dyn Fn() -> anyhow::Result<()> + Send + Sync>,
    statuses: Arc<dyn Fn() -> Vec<crate::file_tail::FileTailStatus> + Send + Sync>,
) -> Self {
    self.file_tail_registry = Some(registry);
    self.file_tail_reconcile = Some(reconcile);
    self.file_tail_statuses = Some(statuses);
    self
}
```

Add module declaration:

```rust
mod file_tails;
```

Create `src/app/services/file_tails.rs`:

```rust
use crate::app::{ServiceError, ServiceResult};
use crate::file_tail::{FileTailRequest, FileTailResponse, FileTailSource};

use super::CortexService;

impl CortexService {
    pub async fn file_tails(&self, req: FileTailRequest) -> ServiceResult<FileTailResponse> {
        req.validate().map_err(ServiceError::InvalidInput)?;
        let registry = self
            .file_tail_registry
            .as_ref()
            .ok_or_else(|| ServiceError::InvalidInput("file-tail registry is not mounted".into()))?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        match req.op {
            crate::file_tail::FileTailOp::List | crate::file_tail::FileTailOp::Status => {}
            crate::file_tail::FileTailOp::Add => {
                registry
                    .upsert(FileTailSource::from_add(req.into_add().map_err(ServiceError::InvalidInput)?, &now))
                    .map_err(|err| ServiceError::Internal(err.to_string()))?;
            }
            crate::file_tail::FileTailOp::Remove => {
                registry
                    .remove(req.id.as_deref().expect("validated id"))
                    .map_err(|err| ServiceError::Internal(err.to_string()))?;
            }
            crate::file_tail::FileTailOp::Enable => {
                registry
                    .set_enabled(req.id.as_deref().expect("validated id"), true, &now)
                    .map_err(|err| ServiceError::InvalidInput(err.to_string()))?;
            }
            crate::file_tail::FileTailOp::Disable => {
                registry
                    .set_enabled(req.id.as_deref().expect("validated id"), false, &now)
                    .map_err(|err| ServiceError::InvalidInput(err.to_string()))?;
            }
        }

        if let Some(reconcile) = &self.file_tail_reconcile {
            reconcile().map_err(|err| ServiceError::Internal(err.to_string()))?;
        }

        let sources = registry
            .list()
            .map_err(|err| ServiceError::Internal(err.to_string()))?;
        let statuses = self
            .file_tail_statuses
            .as_ref()
            .map(|statuses| statuses())
            .unwrap_or_default();
        Ok(FileTailResponse { sources, statuses })
    }
}
```

- [ ] **Step 5: Wire runtime supervisor**

Modify `src/runtime.rs`:

```rust
use crate::file_tail::{FileTailRegistry, FileTailSupervisor};
```

Add field:

```rust
file_tail_supervisor: FileTailSupervisor,
```

In `RuntimeCore::load` after `ingest` creation:

```rust
let file_tail_registry = Arc::new(FileTailRegistry::new(
    FileTailRegistry::path_from_storage_db(&config.storage.db_path),
));
let file_tail_token = CancellationToken::new();
let file_tail_supervisor =
    FileTailSupervisor::new(Arc::clone(&file_tail_registry), ingest.clone(), file_tail_token.clone());
let reconcile_supervisor = file_tail_supervisor.clone();
let status_supervisor = file_tail_supervisor.clone();
let service = CortexService::new(Arc::clone(&pool), config.storage.clone()).with_file_tail_control(
    file_tail_registry,
    Arc::new(move || reconcile_supervisor.reconcile()),
    Arc::new(move || status_supervisor.statuses()),
);
```

Add `file_tail: Option<JoinHandle<()>>` to `MaintenanceHandles`.

In `spawn_maintenance_tasks`, add:

```rust
let file_tail = {
    let supervisor = self.file_tail_supervisor.clone();
    let token = token.clone();
    Some(tokio::spawn(async move {
        if let Err(err) = supervisor.reconcile() {
            tracing::warn!(error = %err, "initial file-tail reconcile failed");
        }
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(err) = supervisor.reconcile() {
                        tracing::warn!(error = %err, "file-tail reconcile failed");
                    }
                }
            }
        }
    }))
};
```

Include `self.file_tail` in shutdown joins.

- [ ] **Step 6: Run test to verify it passes**

Run:

```bash
cargo test file_tails_add_list_disable_enable_remove_round_trip --lib
cargo test runtime --lib
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/runtime.rs src/app.rs src/app/models.rs src/app/models/ops.rs src/app/services.rs src/app/services/file_tails.rs src/app/service_tests.rs src/runtime_tests.rs
git commit -m "feat: manage file-tail sources at runtime"
```

---

### Task 5: Add MCP Action

**Files:**
- Modify: `src/mcp/actions.rs`
- Modify: `src/mcp/tools.rs`
- Modify: `src/mcp/schemas.rs`
- Test: `src/mcp/tools_tests.rs`
- Test: `src/mcp/schemas_tests.rs`

- [ ] **Step 1: Write failing MCP tests**

Add to `src/mcp/tools_tests.rs`:

```rust
#[tokio::test]
async fn file_tails_action_requires_admin_scope() {
    let spec = crate::mcp::actions::ACTION_SPECS
        .iter()
        .find(|spec| spec.name == "file_tails")
        .expect("file_tails registered");
    assert_eq!(spec.scope, crate::mcp::actions::Scope::Admin);
    assert_eq!(spec.cost.as_str(), "write");
}
```

Add to `src/mcp/schemas_tests.rs`:

```rust
#[test]
fn schema_includes_file_tails_action() {
    let tool = super::tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "cortex")
        .expect("cortex tool");
    let schema = serde_json::to_value(tool.input_schema).unwrap();
    assert!(schema.to_string().contains("file_tails"));
    assert!(schema.to_string().contains("op=list|add|remove|enable|disable|status"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test file_tails_action_requires_admin_scope schema_includes_file_tails_action --lib
```

Expected: FAIL because action and schema text are absent.

- [ ] **Step 3: Register action and tool handler**

Modify `src/mcp/actions.rs`:

```rust
FileTails,
```

Add action spec near admin actions:

```rust
action_spec!(
    "file_tails",
    Admin,
    "Manage Cortex-owned file-tail ingest sources",
    Write,
    FileTails
),
```

Modify `src/mcp/tools.rs` dispatch:

```rust
H::FileTails => tool_file_tails(state, args, auth).await,
```

Add handler:

```rust
async fn tool_file_tails(
    state: &AppState,
    args: Value,
    _auth: Option<&AuthContext>,
) -> anyhow::Result<Value> {
    let req: crate::app::FileTailRequest = action_payload(args, "file_tails")?;
    let resp = state.service.file_tails(req).await?;
    Ok(serde_json::to_value(resp)?)
}
```

Modify `src/mcp/schemas.rs` properties for `op`, `id`, `path`, `tag`, `hostname`, `facility`, `severity`, `start_at_end` with descriptions:

```rust
"op": {
  "type": "string",
  "description": "For action=file_tails: op=list|add|remove|enable|disable|status."
}
```

- [ ] **Step 4: Run MCP tests**

Run:

```bash
cargo test file_tails_action_requires_admin_scope schema_includes_file_tails_action --lib
cargo test mcp --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/mcp/actions.rs src/mcp/tools.rs src/mcp/schemas.rs src/mcp/tools_tests.rs src/mcp/schemas_tests.rs
git commit -m "feat: expose file-tail management over MCP"
```

---

### Task 6: Add REST API And CLI

**Files:**
- Modify: `src/api.rs`
- Modify: `src/api_tests.rs`
- Modify: `src/cli/http_client.rs`
- Modify: `src/cli/args.rs`
- Modify: `src/cli/parse.rs`
- Modify: `src/cli/run.rs`
- Modify: `src/cli/dispatch.rs`
- Create: `src/cli/commands/file_tails.rs`
- Modify: `src/cli/commands.rs`
- Test: `src/cli/parse_tests.rs`
- Test: `src/cli/dispatch_tests.rs`

- [ ] **Step 1: Write failing parse tests**

Add to `src/cli/parse_tests.rs`:

```rust
#[test]
fn parses_file_tail_add() {
    let command = parse_command(vec![
        "file-tail".into(),
        "add".into(),
        "--id".into(),
        "swag-access".into(),
        "--path".into(),
        "/mnt/appdata/swag/log/nginx/access.log".into(),
        "--tag".into(),
        "swag-access".into(),
        "--host".into(),
        "edgehost".into(),
        "--facility".into(),
        "local4".into(),
        "--severity".into(),
        "info".into(),
        "--from-start".into(),
        "--json".into(),
    ])
    .unwrap();

    assert_eq!(
        format!("{command:?}"),
        "FileTail(Add(FileTailAddArgs { id: \"swag-access\", path: \"/mnt/appdata/swag/log/nginx/access.log\", tag: \"swag-access\", hostname: Some(\"edgehost\"), facility: Some(\"local4\"), severity: Some(\"info\"), start_at_end: false, json: true }))"
    );
}

#[test]
fn parses_file_tail_list() {
    let command = parse_command(vec!["file-tail".into(), "list".into(), "--json".into()]).unwrap();
    assert_eq!(
        format!("{command:?}"),
        "FileTail(List(FileTailListArgs { json: true }))"
    );
}
```

- [ ] **Step 2: Run parse tests to verify they fail**

Run:

```bash
cargo test parses_file_tail_add parses_file_tail_list --lib
```

Expected: compile fails because CLI variants do not exist.

- [ ] **Step 3: Add CLI args and parser**

Modify `src/cli/args.rs`:

```rust
FileTail(FileTailCommand),
```

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileTailCommand {
    List(FileTailListArgs),
    Status(FileTailListArgs),
    Add(FileTailAddArgs),
    Remove(FileTailIdArgs),
    Enable(FileTailIdArgs),
    Disable(FileTailIdArgs),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileTailListArgs {
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileTailIdArgs {
    pub id: String,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileTailAddArgs {
    pub id: String,
    pub path: String,
    pub tag: String,
    pub hostname: Option<String>,
    pub facility: Option<String>,
    pub severity: Option<String>,
    pub start_at_end: bool,
    pub json: bool,
}
```

Create `src/cli/commands/file_tails.rs`:

```rust
use anyhow::{Result, anyhow, bail};

use crate::cli::{
    CliCommand, FileTailAddArgs, FileTailCommand, FileTailIdArgs, FileTailListArgs, suggest,
};

pub(crate) fn parse_file_tail(args: &[String]) -> Result<CliCommand> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("file-tail subcommand is required"))?;
    match command.as_str() {
        "list" => Ok(CliCommand::FileTail(FileTailCommand::List(parse_list(rest)?))),
        "status" => Ok(CliCommand::FileTail(FileTailCommand::Status(parse_list(rest)?))),
        "add" => Ok(CliCommand::FileTail(FileTailCommand::Add(parse_add(rest)?))),
        "remove" => Ok(CliCommand::FileTail(FileTailCommand::Remove(parse_id(rest)?))),
        "enable" => Ok(CliCommand::FileTail(FileTailCommand::Enable(parse_id(rest)?))),
        "disable" => Ok(CliCommand::FileTail(FileTailCommand::Disable(parse_id(rest)?))),
        _ => bail!(
            "{}",
            suggest::unknown_command(
                "file-tail subcommand",
                command,
                &["list", "status", "add", "remove", "enable", "disable"],
            )
        ),
    }
}

fn parse_list(args: &[String]) -> Result<FileTailListArgs> {
    let mut out = FileTailListArgs { json: false };
    for arg in args {
        match arg.as_str() {
            "--json" => out.json = true,
            "--help" | "-h" => bail!("{}", usage()),
            other => bail!("{}", suggest::unknown_option("file-tail list", other, &["--json"])),
        }
    }
    Ok(out)
}

fn parse_id(args: &[String]) -> Result<FileTailIdArgs> {
    let mut id = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => {
                i += 1;
                id = Some(required(args, i, "--id")?);
            }
            "--json" => json = true,
            other => bail!("{}", suggest::unknown_option("file-tail", other, &["--id", "--json"])),
        }
        i += 1;
    }
    Ok(FileTailIdArgs {
        id: id.ok_or_else(|| anyhow!("--id is required"))?,
        json,
    })
}

fn parse_add(args: &[String]) -> Result<FileTailAddArgs> {
    let mut out = FileTailAddArgs {
        id: String::new(),
        path: String::new(),
        tag: String::new(),
        hostname: None,
        facility: None,
        severity: None,
        start_at_end: true,
        json: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => {
                i += 1;
                out.id = required(args, i, "--id")?;
            }
            "--path" => {
                i += 1;
                out.path = required(args, i, "--path")?;
            }
            "--tag" => {
                i += 1;
                out.tag = required(args, i, "--tag")?;
            }
            "--host" => {
                i += 1;
                out.hostname = Some(required(args, i, "--host")?);
            }
            "--facility" => {
                i += 1;
                out.facility = Some(required(args, i, "--facility")?);
            }
            "--severity" => {
                i += 1;
                out.severity = Some(required(args, i, "--severity")?);
            }
            "--from-start" => out.start_at_end = false,
            "--json" => out.json = true,
            other => bail!("{}", suggest::unknown_option("file-tail add", other, &[
                "--id", "--path", "--tag", "--host", "--facility", "--severity", "--from-start", "--json",
            ])),
        }
        i += 1;
    }
    if out.id.is_empty() || out.path.is_empty() || out.tag.is_empty() {
        bail!("file-tail add requires --id, --path, and --tag");
    }
    Ok(out)
}

fn required(args: &[String], index: usize, flag: &str) -> Result<String> {
    let value = args
        .get(index)
        .ok_or_else(|| anyhow!("{flag} requires a value"))?;
    if value.trim().is_empty() || value.starts_with('-') {
        bail!("{flag} requires a value");
    }
    Ok(value.clone())
}

fn usage() -> &'static str {
    "Usage: cortex file-tail list [--json]\n       cortex file-tail add --id ID --path PATH --tag TAG [--host HOST] [--facility FACILITY] [--severity SEVERITY] [--from-start] [--json]\n       cortex file-tail remove --id ID [--json]\n       cortex file-tail enable --id ID [--json]\n       cortex file-tail disable --id ID [--json]"
}
```

Modify `src/cli/commands.rs`:

```rust
pub(crate) mod file_tails;
```

Modify `src/cli/parse.rs`:

```rust
"file-tail",
```

and:

```rust
"file-tail" => commands::file_tails::parse_file_tail(rest),
```

- [ ] **Step 4: Add API route and HTTP client**

Modify `src/api.rs` route list:

```rust
.route("/api/file-tails", post(file_tails))
```

Add handler:

```rust
async fn file_tails(
    State(state): State<ApiState>,
    Json(req): Json<crate::app::FileTailRequest>,
) -> impl IntoResponse {
    respond(state.service.file_tails(req).await)
}
```

Modify `src/cli/http_client.rs`:

```rust
pub async fn file_tails(
    &self,
    req: &crate::app::FileTailRequest,
) -> Result<crate::app::FileTailResponse> {
    self.post_json("/api/file-tails", req).await
}
```

- [ ] **Step 5: Add CLI dispatch**

Modify imports in `src/cli/run.rs`:

```rust
FileTailCommand,
```

Add match arm:

```rust
CliCommand::FileTail(command) => dispatch::run_file_tail(&mode, command).await,
```

Modify `src/cli/dispatch.rs`:

```rust
pub(crate) async fn run_file_tail(
    mode: &CliMode,
    command: super::FileTailCommand,
) -> Result<()> {
    let (req, json) = match command {
        super::FileTailCommand::List(args) => (
            cortex::app::FileTailRequest {
                op: cortex::app::FileTailOp::List,
                id: None,
                path: None,
                tag: None,
                hostname: None,
                facility: None,
                severity: None,
                start_at_end: None,
            },
            args.json,
        ),
        super::FileTailCommand::Status(args) => (
            cortex::app::FileTailRequest {
                op: cortex::app::FileTailOp::Status,
                id: None,
                path: None,
                tag: None,
                hostname: None,
                facility: None,
                severity: None,
                start_at_end: None,
            },
            args.json,
        ),
        super::FileTailCommand::Add(args) => (
            cortex::app::FileTailRequest {
                op: cortex::app::FileTailOp::Add,
                id: Some(args.id),
                path: Some(args.path),
                tag: Some(args.tag),
                hostname: args.hostname,
                facility: args.facility,
                severity: args.severity,
                start_at_end: Some(args.start_at_end),
            },
            args.json,
        ),
        super::FileTailCommand::Remove(args) => id_request(cortex::app::FileTailOp::Remove, args),
        super::FileTailCommand::Enable(args) => id_request(cortex::app::FileTailOp::Enable, args),
        super::FileTailCommand::Disable(args) => id_request(cortex::app::FileTailOp::Disable, args),
    };
    let response = match mode {
        CliMode::Local(service) => service.file_tails(req).await?,
        CliMode::Http(client) => http_or_cancel(client.file_tails(&req)).await?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        for source in response.sources {
            println!(
                "{}\t{}\t{}\t{}",
                source.id,
                if source.enabled { "enabled" } else { "disabled" },
                source.tag,
                source.path
            );
        }
    }
    Ok(())
}

fn id_request(
    op: cortex::app::FileTailOp,
    args: super::FileTailIdArgs,
) -> (cortex::app::FileTailRequest, bool) {
    (
        cortex::app::FileTailRequest {
            op,
            id: Some(args.id),
            path: None,
            tag: None,
            hostname: None,
            facility: None,
            severity: None,
            start_at_end: None,
        },
        args.json,
    )
}
```

- [ ] **Step 6: Run CLI/API tests**

Run:

```bash
cargo test parses_file_tail_add parses_file_tail_list --lib
cargo test api --lib
cargo test dispatch --lib
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/api.rs src/api_tests.rs src/cli/http_client.rs src/cli/args.rs src/cli/parse.rs src/cli/run.rs src/cli/dispatch.rs src/cli/commands.rs src/cli/commands/file_tails.rs src/cli/parse_tests.rs src/cli/dispatch_tests.rs
git commit -m "feat: expose file-tail management over api and cli"
```

---

### Task 7: Add Docs, Defaults, Version Bump, And Homelab Recipes

**Files:**
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Modify: `docs/CLI.md`
- Modify: `docs/api.md`
- Modify: `docs/mcp/SCHEMA.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/contracts/source-kinds.md`
- Modify: `.env.example`
- Modify: `config.toml`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `server.json`
- Modify: `mcpb/manifest.json`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Write docs update**

Add this section to `docs/CONFIG.md`:

```markdown
## Managed File-Tail Sources

Cortex can tail local log files directly and ingest appended lines through the
same writer/enrichment path as syslog, Docker, and OTLP. Sources are stored in
`<data-dir>/file-tails.json`, where `<data-dir>` is the parent directory of
`CORTEX_DB_PATH`.

Use this for logs that do not naturally reach journald or container stdout,
such as SWAG nginx access/error logs, SWAG fail2ban logs, Authelia file logs,
and AdGuard query logs.

```bash
cortex file-tail add \
  --id swag-access \
  --path /mnt/appdata/swag/log/nginx/access.log \
  --tag swag-access \
  --host edgehost \
  --facility local4

cortex file-tail add \
  --id swag-error \
  --path /mnt/appdata/swag/log/nginx/error.log \
  --tag swag-error \
  --host edgehost \
  --facility local4 \
  --severity warning

cortex file-tail add \
  --id fail2ban \
  --path /mnt/appdata/swag/log/fail2ban/fail2ban.log \
  --tag fail2ban \
  --host edgehost \
  --facility local5

cortex file-tail add \
  --id authelia \
  --path /mnt/appdata/authelia/logs/authelia.log \
  --tag authelia \
  --host edgehost \
  --facility local5

cortex file-tail add \
  --id adguard-query \
  --path /mnt/appdata/adguard/var/data/querylog.json \
  --tag adguard-query \
  --host edgehost \
  --facility local6
```

`--from-start` ingests existing file contents. The default starts at EOF so
adding a source does not backfill a large historic log unexpectedly.
```

Add CLI docs to `docs/CLI.md`:

```markdown
## `cortex file-tail`

Manage Cortex-owned file-tail ingest sources.

```bash
cortex file-tail list [--json]
cortex file-tail status [--json]
cortex file-tail add --id ID --path PATH --tag TAG [--host HOST] [--facility FACILITY] [--severity SEVERITY] [--from-start] [--json]
cortex file-tail remove --id ID [--json]
cortex file-tail enable --id ID [--json]
cortex file-tail disable --id ID [--json]
```

The command maps to MCP action `file_tails` and REST `POST /api/file-tails`.
```

Add API docs to `docs/api.md`:

```markdown
### `POST /api/file-tails`

Admin endpoint for Cortex-owned file-tail ingest sources.

Request:

```json
{
  "op": "add",
  "id": "swag-access",
  "path": "/mnt/appdata/swag/log/nginx/access.log",
  "tag": "swag-access",
  "hostname": "edgehost",
  "facility": "local4",
  "severity": "info",
  "start_at_end": true
}
```

`op` may be `list`, `add`, `remove`, `enable`, `disable`, or `status`.
```

- [ ] **Step 2: Bump version**

Run:

```bash
scripts/bump-version.sh minor
```

Expected: version-bearing files move from `1.19.0` to `1.20.0`.

Add to `CHANGELOG.md` under `1.20.0`:

```markdown
- Added managed file-tail ingest sources with CLI, REST API, and MCP control.
- Added `file-tail` source kind for rows ingested from local log files.
- Documented SWAG, fail2ban, Authelia, and AdGuard file-tail recipes for replacing rsyslog `imfile` drop-ins.
```

- [ ] **Step 3: Run docs/version checks**

Run:

```bash
cargo test source_kind --lib
bash scripts/check-version-sync.sh
```

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
git add README.md CLAUDE.md docs/CLI.md docs/api.md docs/mcp/SCHEMA.md docs/CONFIG.md docs/contracts/source-kinds.md .env.example config.toml Cargo.toml Cargo.lock server.json mcpb/manifest.json CHANGELOG.md
git commit -m "docs: document managed file-tail ingest"
```

---

### Task 8: End-To-End Verification

**Files:**
- No planned source edits unless a verification failure identifies a bug.

- [ ] **Step 1: Run full quality gates**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Expected: PASS.

- [ ] **Step 2: Run local live smoke with a temporary log file**

Run:

```bash
tmpdir=$(mktemp -d)
export CORTEX_DB_PATH="$tmpdir/cortex.db"
export CORTEX_API_TOKEN="test-token"
target/debug/cortex serve mcp --no-auth >"$tmpdir/cortex.log" 2>&1 &
pid=$!
sleep 2
target/debug/cortex file-tail add --id smoke-file --path "$tmpdir/app.log" --tag smoke-app --host smoke-host --from-start --json
printf 'hello from managed file tail\n' >> "$tmpdir/app.log"
sleep 2
target/debug/cortex search '"hello from managed file tail"' --json
kill "$pid"
```

Expected: search JSON contains one row with:

```json
{
  "hostname": "smoke-host",
  "app_name": "smoke-app",
  "message": "hello from managed file tail"
}
```

- [ ] **Step 3: Verify MCP action with mcporter**

Run with the server from Step 2 still running, or restart it:

```bash
mcporter call --config config/mcporter.json cortex.cortex action=file_tails op=list
```

Expected: response includes `smoke-file` in `sources`.

- [ ] **Step 4: Verify API action**

Run:

```bash
curl -sS -X POST http://127.0.0.1:3100/api/file-tails \
  -H 'Authorization: Bearer test-token' \
  -H 'Content-Type: application/json' \
  -d '{"op":"status"}' | jq .
```

Expected: response contains `sources` and `statuses` arrays.

- [ ] **Step 5: Commit any verification fixes**

If any fixes were required:

```bash
git add <fixed-files>
git commit -m "fix: stabilize managed file-tail ingest"
```

If no fixes were required, do not create an empty commit.

- [ ] **Step 6: Final branch hygiene**

Run:

```bash
git status --short --branch
bd update syslog-mcp-6y96m --status in_progress
```

Expected: worktree is clean and branch is ahead of `main` by the implementation commits.

---

## Self-Review

**Spec coverage:** The plan covers managed file-tail registration, ingestion, persistence, runtime task control, CLI, REST API, MCP, docs, and verification. It explicitly includes the old specialty sources: SWAG access/error, SWAG fail2ban, Authelia, and AdGuard.

**Placeholder scan:** No placeholder markers or vague test instructions remain. Each task has concrete file paths, commands, and expected results.

**Type consistency:** The shared DTO names are consistent across tasks: `FileTailRequest`, `FileTailOp`, `FileTailSource`, `FileTailStatus`, and `FileTailResponse`. The single MCP action name is `file_tails`; the CLI command is `file-tail`; the REST endpoint is `POST /api/file-tails`; the source kind wire value is `file-tail`.
