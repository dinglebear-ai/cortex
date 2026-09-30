# Compose Lifecycle CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build safe `syslog compose ...` lifecycle commands that diagnose and manage the syslog-mcp Compose deployment without loading the SQLite query runtime.

**Architecture:** Add a shared `compose` library module that owns target discovery, mutation preflight, Docker/Compose subprocess execution, bounded output capture, and MCP-safe redaction. The CLI gets full local lifecycle control; MCP gets only redacted read-only diagnostics for the canonical syslog-mcp target.

**Tech Stack:** Rust 1.86, Tokio, serde/serde_json, std process APIs, Docker CLI, Docker Compose CLI, existing RMCP action-dispatch server.

---

## File Structure

- Create `src/compose.rs`: shared compose models, target resolution, runner traits, subprocess runner, mutation preflight, redaction, MCP projection, and tests hook.
- Create `src/compose_tests.rs`: focused unit tests with fake inspectors/runners; no live Docker daemon required.
- Modify `src/lib.rs`: export `pub mod compose;`.
- Modify `src/cli.rs`: add `CliCommand::Compose`, parse `syslog compose ...`, route to shared compose service, print human/JSON results.
- Modify `src/main.rs`: include `compose` in top-level mode parsing and run compose commands without `RuntimeCore::load_query_only()`.
- Modify `src/main_tests.rs`: cover top-level `compose` parsing.
- Modify `src/cli_tests.rs`: cover compose parsing and option validation.
- Modify `src/mcp.rs`: add a compose diagnostics concurrency limiter to `AppState` if needed by implementation.
- Modify `src/mcp/schemas.rs`: add `compose_status` and `compose_doctor`; do not add `compose_config`.
- Modify `src/mcp/tools.rs`: dispatch read-only compose actions, reject target overrides, return `ComposeMcpStatus`.
- Modify `src/mcp/tools_tests.rs`: cover MCP compose output projection and target override rejection.
- Modify `src/mcp/rmcp_server.rs`: add compose actions to the read-scope map only after the MCP-safe DTO is in place.
- Modify `src/mcp/rmcp_server_tests.rs`: verify compose actions require the existing read scope and unknown/mutating compose actions remain denied.
- Modify docs after code passes: `README.md`, `docs/mcp/DEPLOY.md`, `docs/runbooks/deploy.md`, and `AGENTS.md` if command lists need updating.

## Task 1: Shared Compose Models and Redaction

**Files:**
- Create: `src/compose.rs`
- Create: `src/compose_tests.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add the module export**

Modify `src/lib.rs` so the public module list includes compose:

```rust
pub mod api;
pub mod app;
pub mod compose;
pub mod config;
pub mod mcp;
pub mod observability;
pub mod otlp;
pub mod runtime;
pub mod scanner;
pub mod syslog;
```

- [ ] **Step 2: Create the first compose model skeleton**

Create `src/compose.rs` with these definitions:

```rust
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeDefaults {
    pub service: String,
    pub container_name: String,
    pub timeout: Duration,
    pub output_limit_bytes: usize,
}

impl Default for ComposeDefaults {
    fn default() -> Self {
        Self {
            service: "syslog-mcp".into(),
            container_name: "syslog-mcp".into(),
            timeout: Duration::from_secs(120),
            output_limit_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeTarget {
    pub project_dir: Option<PathBuf>,
    pub compose_file: Option<PathBuf>,
    pub project_name: Option<String>,
    pub service: Option<String>,
    pub container_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSource {
    Explicit,
    LiveContainerLabels,
    CurrentWorkingDirectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetConfidence {
    Confirmed,
    Ambiguous,
    Unsafe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Unsafe,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedComposeTarget {
    pub target: ComposeTargetSummary,
    pub source: TargetSource,
    pub confidence: TargetConfidence,
    pub diagnostics: Vec<ComposeDiagnostic>,
    pub compose_files: Vec<PathBuf>,
    pub compose_working_dir: Option<PathBuf>,
    pub compose_project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeTargetSummary {
    pub project_dir: Option<PathBuf>,
    pub compose_file: Option<PathBuf>,
    pub project_name: Option<String>,
    pub service: String,
    pub container_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MountInfo {
    pub source: Option<PathBuf>,
    pub target: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortInfo {
    pub private_port: u16,
    pub public_port: Option<u16>,
    pub protocol: String,
    pub host_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemdStatus {
    pub unit: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeStatus {
    pub container_name: String,
    pub container_id: Option<String>,
    pub status: Option<String>,
    pub health: Option<String>,
    pub image: Option<String>,
    pub image_id: Option<String>,
    pub compose_project: Option<String>,
    pub compose_working_dir: Option<PathBuf>,
    pub compose_files: Vec<PathBuf>,
    pub service: Option<String>,
    pub data_mounts: Vec<MountInfo>,
    pub ports: Vec<PortInfo>,
    pub systemd: Option<SystemdStatus>,
    pub diagnostics: Vec<ComposeDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicPortSummary {
    pub port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposeOwnershipState {
    ComposeOwned,
    OwnerMismatch,
    SystemdOwned,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposeRuntimeState {
    Healthy,
    Degraded,
    Stopped,
    DockerUnavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeMcpDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeMcpStatus {
    pub container_name: String,
    pub ownership: ComposeOwnershipState,
    pub runtime_state: ComposeRuntimeState,
    pub health: Option<String>,
    pub published_ports: Vec<PublicPortSummary>,
    pub diagnostics: Vec<ComposeMcpDiagnostic>,
}

pub fn redact_sensitive(input: &str) -> String {
    let sensitive = [
        "token",
        "secret",
        "key",
        "password",
        "client_secret",
        "authorization",
    ];
    input
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if sensitive.iter().any(|term| lower.contains(term)) {
                "[REDACTED]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn mcp_projection(status: &ComposeStatus) -> ComposeMcpStatus {
    let ownership = if status
        .diagnostics
        .iter()
        .any(|d| d.code == "owner_mismatch")
    {
        ComposeOwnershipState::OwnerMismatch
    } else if status.systemd.as_ref().is_some_and(|s| s.active) {
        ComposeOwnershipState::SystemdOwned
    } else if status.compose_project.is_some() {
        ComposeOwnershipState::ComposeOwned
    } else {
        ComposeOwnershipState::Unknown
    };

    let runtime_state = match status.health.as_deref() {
        Some("healthy") => ComposeRuntimeState::Healthy,
        Some("unhealthy") => ComposeRuntimeState::Degraded,
        _ if status.status.as_deref().is_some_and(|s| s.contains("Exited")) => {
            ComposeRuntimeState::Stopped
        }
        _ => ComposeRuntimeState::Unknown,
    };

    ComposeMcpStatus {
        container_name: status.container_name.clone(),
        ownership,
        runtime_state,
        health: status.health.clone(),
        published_ports: status
            .ports
            .iter()
            .filter_map(|port| {
                port.public_port.map(|public| PublicPortSummary {
                    port: public,
                    protocol: port.protocol.clone(),
                })
            })
            .collect(),
        diagnostics: status
            .diagnostics
            .iter()
            .map(|d| ComposeMcpDiagnostic {
                severity: d.severity.clone(),
                code: d.code.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;
```

- [ ] **Step 3: Add model and redaction tests**

Create `src/compose_tests.rs`:

```rust
use super::*;

#[test]
fn redacts_sensitive_lines() {
    let input = "ok=true\nSYSLOG_MCP_TOKEN=abc\nclient_secret = \"secret\"\nport=3100";
    let redacted = redact_sensitive(input);
    assert!(redacted.contains("ok=true"));
    assert!(redacted.contains("port=3100"));
    assert!(!redacted.contains("abc"));
    assert!(!redacted.contains("client_secret"));
    assert_eq!(redacted.matches("[REDACTED]").count(), 2);
}

#[test]
fn mcp_projection_omits_host_paths_and_image_ids() {
    let status = ComposeStatus {
        container_name: "syslog-mcp".into(),
        container_id: Some("container-id".into()),
        status: Some("Up 1 minute".into()),
        health: Some("healthy".into()),
        image: Some("ghcr.io/jmagar/syslog-mcp:latest".into()),
        image_id: Some("sha256:secret-image-id".into()),
        compose_project: Some("syslog-jmagar-lab".into()),
        compose_working_dir: Some(PathBuf::from("/home/jmagar/private")),
        compose_files: vec![PathBuf::from("/home/jmagar/private/docker-compose.yml")],
        service: Some("syslog-mcp".into()),
        data_mounts: vec![MountInfo {
            source: Some(PathBuf::from("/home/jmagar/private/data")),
            target: "/data".into(),
            kind: "bind".into(),
        }],
        ports: vec![PortInfo {
            private_port: 3100,
            public_port: Some(3100),
            protocol: "tcp".into(),
            host_ip: Some("0.0.0.0".into()),
        }],
        systemd: None,
        diagnostics: vec![],
    };

    let projected = mcp_projection(&status);
    let json = serde_json::to_string(&projected).unwrap();
    assert_eq!(projected.ownership, ComposeOwnershipState::ComposeOwned);
    assert_eq!(projected.runtime_state, ComposeRuntimeState::Healthy);
    assert!(json.contains("3100"));
    assert!(!json.contains("/home/jmagar"));
    assert!(!json.contains("secret-image-id"));
    assert!(!json.contains("/data"));
}
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run: `cargo test compose::tests --lib`

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/compose.rs src/compose_tests.rs
git commit -m "feat: add compose lifecycle models"
```

## Task 2: Target Resolution and Mutation Safety

**Files:**
- Modify: `src/compose.rs`
- Modify: `src/compose_tests.rs`

- [ ] **Step 1: Add inspector models and traits**

Append to `src/compose.rs`:

```rust
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub status: Option<String>,
    pub health: Option<String>,
    pub image: Option<String>,
    pub image_id: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub mounts: Vec<MountInfo>,
    pub ports: Vec<PortInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerInfo {
    pub port: u16,
    pub process: Option<String>,
    pub belongs_to_target: bool,
}

pub trait DockerInspect {
    fn inspect_container(&self, name: &str) -> Result<Option<ContainerInfo>>;
    fn find_candidates(&self, service: &str, container_name: &str) -> Result<Vec<ContainerInfo>>;
    fn systemd_status(&self, unit: &str) -> Result<Option<SystemdStatus>>;
    fn listeners(&self, ports: &[u16]) -> Result<Vec<ListenerInfo>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeMutation {
    Up,
    Down,
    Restart,
    Pull,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationOptions {
    pub dry_run: bool,
    pub allow_cwd_target: bool,
    pub allow_foreign_project: bool,
    pub yes: bool,
    pub non_interactive: bool,
}

pub struct ComposeService<I, R> {
    inspector: I,
    runner: R,
    defaults: ComposeDefaults,
}

impl<I, R> ComposeService<I, R> {
    pub fn new(inspector: I, runner: R, defaults: ComposeDefaults) -> Self {
        Self {
            inspector,
            runner,
            defaults,
        }
    }
}
```

- [ ] **Step 2: Implement label extraction**

Add these helpers to `src/compose.rs`:

```rust
fn label<'a>(info: &'a ContainerInfo, key: &str) -> Option<&'a str> {
    info.labels.get(key).map(String::as_str)
}

fn split_compose_files(value: Option<&str>) -> Vec<PathBuf> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn target_from_container(info: &ContainerInfo, defaults: &ComposeDefaults) -> ResolvedComposeTarget {
    let service = label(info, "com.docker.compose.service")
        .unwrap_or(defaults.service.as_str())
        .to_string();
    let container_name = info.name.trim_start_matches('/').to_string();
    let compose_working_dir = label(info, "com.docker.compose.project.working_dir").map(PathBuf::from);
    let compose_files = split_compose_files(label(info, "com.docker.compose.project.config_files"));
    ResolvedComposeTarget {
        target: ComposeTargetSummary {
            project_dir: compose_working_dir.clone(),
            compose_file: compose_files.first().cloned(),
            project_name: label(info, "com.docker.compose.project").map(str::to_string),
            service,
            container_name,
        },
        source: TargetSource::LiveContainerLabels,
        confidence: TargetConfidence::Confirmed,
        diagnostics: Vec::new(),
        compose_files,
        compose_working_dir,
        compose_project: label(info, "com.docker.compose.project").map(str::to_string),
    }
}
```

- [ ] **Step 3: Implement safe target resolution**

Add to `src/compose.rs`:

```rust
impl<I: DockerInspect, R> ComposeService<I, R> {
    pub fn resolve_target(&self, requested: &ComposeTarget) -> Result<ResolvedComposeTarget> {
        let service = requested
            .service
            .clone()
            .unwrap_or_else(|| self.defaults.service.clone());
        let container_name = requested
            .container_name
            .clone()
            .unwrap_or_else(|| self.defaults.container_name.clone());

        if requested.compose_file.is_some() || requested.project_dir.is_some() {
            return Ok(ResolvedComposeTarget {
                target: ComposeTargetSummary {
                    project_dir: requested.project_dir.clone(),
                    compose_file: requested.compose_file.clone(),
                    project_name: requested.project_name.clone(),
                    service,
                    container_name,
                },
                source: TargetSource::Explicit,
                confidence: TargetConfidence::Confirmed,
                diagnostics: Vec::new(),
                compose_files: requested.compose_file.clone().into_iter().collect(),
                compose_working_dir: requested.project_dir.clone(),
                compose_project: requested.project_name.clone(),
            });
        }

        if let Some(info) = self.inspector.inspect_container(&container_name)? {
            return Ok(target_from_container(&info, &self.defaults));
        }

        let candidates = self.inspector.find_candidates(&service, &container_name)?;
        if candidates.len() == 1 {
            return Ok(target_from_container(&candidates[0], &self.defaults));
        }
        if candidates.len() > 1 {
            return Ok(ResolvedComposeTarget {
                target: ComposeTargetSummary {
                    project_dir: None,
                    compose_file: None,
                    project_name: requested.project_name.clone(),
                    service,
                    container_name,
                },
                source: TargetSource::LiveContainerLabels,
                confidence: TargetConfidence::Ambiguous,
                diagnostics: vec![ComposeDiagnostic {
                    severity: DiagnosticSeverity::Unsafe,
                    code: "multiple_compose_candidates".into(),
                    message: format!("found {} candidate syslog-mcp containers", candidates.len()),
                }],
                compose_files: Vec::new(),
                compose_working_dir: None,
                compose_project: requested.project_name.clone(),
            });
        }

        let cwd_file = std::env::current_dir()?.join("docker-compose.yml");
        if cwd_file.exists() {
            return Ok(ResolvedComposeTarget {
                target: ComposeTargetSummary {
                    project_dir: Some(std::env::current_dir()?),
                    compose_file: Some(cwd_file.clone()),
                    project_name: requested.project_name.clone(),
                    service,
                    container_name,
                },
                source: TargetSource::CurrentWorkingDirectory,
                confidence: TargetConfidence::Unsafe,
                diagnostics: vec![ComposeDiagnostic {
                    severity: DiagnosticSeverity::Unsafe,
                    code: "cwd_fallback_requires_confirmation".into(),
                    message: "cwd docker-compose.yml is not enough for mutation without --allow-cwd-target".into(),
                }],
                compose_files: vec![cwd_file],
                compose_working_dir: Some(std::env::current_dir()?),
                compose_project: requested.project_name.clone(),
            });
        }

        Err(anyhow!("could not resolve syslog-mcp compose target"))
    }
}
```

- [ ] **Step 4: Implement mutation preflight**

Add to `src/compose.rs`:

```rust
impl<I: DockerInspect, R> ComposeService<I, R> {
    pub fn preflight_mutation(
        &self,
        mutation: ComposeMutation,
        target: &ResolvedComposeTarget,
        options: &MutationOptions,
    ) -> Result<()> {
        if target.confidence == TargetConfidence::Ambiguous {
            return Err(anyhow!("refusing mutation: target is ambiguous"));
        }
        if target.source == TargetSource::CurrentWorkingDirectory && !options.allow_cwd_target {
            return Err(anyhow!(
                "refusing mutation: cwd target requires --allow-cwd-target"
            ));
        }
        if target.target.project_name.is_some()
            && target.target.project_dir.is_none()
            && target.target.compose_file.is_none()
            && target.source != TargetSource::LiveContainerLabels
        {
            return Err(anyhow!(
                "refusing mutation: --project-name alone is not a safe target"
            ));
        }
        for file in &target.compose_files {
            if !file.exists() {
                return Err(anyhow!("refusing mutation: compose file does not exist: {}", file.display()));
            }
        }

        let systemd = self.inspector.systemd_status("syslog-mcp.service")?;
        let listeners = self.inspector.listeners(&[1514, 3100])?;
        let non_target_listener = listeners.iter().any(|l| !l.belongs_to_target);
        let systemd_active = systemd.as_ref().is_some_and(|s| s.active);

        match mutation {
            ComposeMutation::Up | ComposeMutation::Restart if systemd_active || non_target_listener => {
                return Err(anyhow!(
                    "refusing mutation: systemd or non-target listener owns syslog ports"
                ));
            }
            ComposeMutation::Down if target.source != TargetSource::LiveContainerLabels => {
                return Err(anyhow!("refusing down: target must be confirmed compose-owned"));
            }
            ComposeMutation::Down if options.non_interactive && !options.yes => {
                return Err(anyhow!("refusing down: --yes is required in non-interactive mode"));
            }
            ComposeMutation::Pull | ComposeMutation::Up | ComposeMutation::Restart | ComposeMutation::Down => {}
        }

        Ok(())
    }
}
```

- [ ] **Step 5: Add fake inspector tests**

Append to `src/compose_tests.rs`:

```rust
use std::collections::BTreeMap;

#[derive(Default)]
struct FakeInspector {
    container: Option<ContainerInfo>,
    candidates: Vec<ContainerInfo>,
    systemd: Option<SystemdStatus>,
    listeners: Vec<ListenerInfo>,
}

impl DockerInspect for FakeInspector {
    fn inspect_container(&self, _name: &str) -> Result<Option<ContainerInfo>> {
        Ok(self.container.clone())
    }

    fn find_candidates(&self, _service: &str, _container_name: &str) -> Result<Vec<ContainerInfo>> {
        Ok(self.candidates.clone())
    }

    fn systemd_status(&self, _unit: &str) -> Result<Option<SystemdStatus>> {
        Ok(self.systemd.clone())
    }

    fn listeners(&self, _ports: &[u16]) -> Result<Vec<ListenerInfo>> {
        Ok(self.listeners.clone())
    }
}

#[derive(Default)]
struct FakeRunner;

fn labelled_container() -> ContainerInfo {
    let mut labels = BTreeMap::new();
    labels.insert("com.docker.compose.project".into(), "syslog-jmagar-lab".into());
    labels.insert(
        "com.docker.compose.project.working_dir".into(),
        "/tmp/syslog-jmagar-lab".into(),
    );
    labels.insert(
        "com.docker.compose.project.config_files".into(),
        "/tmp/syslog-jmagar-lab/docker-compose.yml".into(),
    );
    labels.insert("com.docker.compose.service".into(), "syslog-mcp".into());
    ContainerInfo {
        id: "abc".into(),
        name: "syslog-mcp".into(),
        status: Some("Up".into()),
        health: Some("healthy".into()),
        image: Some("ghcr.io/jmagar/syslog-mcp:latest".into()),
        image_id: Some("sha256:abc".into()),
        labels,
        mounts: Vec::new(),
        ports: Vec::new(),
    }
}

#[test]
fn resolves_live_container_labels() {
    let service = ComposeService::new(
        FakeInspector {
            container: Some(labelled_container()),
            ..Default::default()
        },
        FakeRunner,
        ComposeDefaults::default(),
    );
    let target = service.resolve_target(&ComposeTarget::default()).unwrap();
    assert_eq!(target.source, TargetSource::LiveContainerLabels);
    assert_eq!(target.confidence, TargetConfidence::Confirmed);
    assert_eq!(target.compose_project.as_deref(), Some("syslog-jmagar-lab"));
}

#[test]
fn project_name_alone_is_rejected_for_mutation() {
    let service = ComposeService::new(FakeInspector::default(), FakeRunner, ComposeDefaults::default());
    let target = ResolvedComposeTarget {
        target: ComposeTargetSummary {
            project_dir: None,
            compose_file: None,
            project_name: Some("syslog".into()),
            service: "syslog-mcp".into(),
            container_name: "syslog-mcp".into(),
        },
        source: TargetSource::Explicit,
        confidence: TargetConfidence::Confirmed,
        diagnostics: Vec::new(),
        compose_files: Vec::new(),
        compose_working_dir: None,
        compose_project: Some("syslog".into()),
    };
    let err = service
        .preflight_mutation(ComposeMutation::Up, &target, &MutationOptions::default())
        .unwrap_err();
    assert!(err.to_string().contains("--project-name alone"));
}

#[test]
fn up_refuses_non_target_listener() {
    let service = ComposeService::new(
        FakeInspector {
            listeners: vec![ListenerInfo {
                port: 3100,
                process: Some("other".into()),
                belongs_to_target: false,
            }],
            ..Default::default()
        },
        FakeRunner,
        ComposeDefaults::default(),
    );
    let target = target_from_container(&labelled_container(), &ComposeDefaults::default());
    let err = service
        .preflight_mutation(ComposeMutation::Up, &target, &MutationOptions::default())
        .unwrap_err();
    assert!(err.to_string().contains("non-target listener"));
}
```

- [ ] **Step 6: Run tests and verify**

Run: `cargo test compose::tests --lib`

Expected: new target-resolution tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/compose.rs src/compose_tests.rs
git commit -m "feat: add compose target safety checks"
```

## Task 3: Subprocess Runner and Compose Invocation Semantics

**Files:**
- Modify: `src/compose.rs`
- Modify: `src/compose_tests.rs`

- [ ] **Step 1: Add command invocation models**

Append to `src/compose.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub timeout: Duration,
    pub output_limit_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimeoutCleanupStatus {
    pub terminate_sent: bool,
    pub kill_sent: bool,
    pub reaped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandOutput {
    pub exit_status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub timeout_cleanup: Option<TimeoutCleanupStatus>,
}

pub trait CommandRunner {
    fn run(&self, invocation: &ComposeInvocation) -> Result<CommandOutput>;
}
```

- [ ] **Step 2: Add invocation builder**

Append to `src/compose.rs`:

```rust
impl<I, R> ComposeService<I, R> {
    pub fn compose_invocation(
        &self,
        target: &ResolvedComposeTarget,
        mutation: ComposeMutation,
    ) -> ComposeInvocation {
        let mut args = Vec::new();
        if let Some(project_dir) = &target.compose_working_dir {
            args.push("--project-directory".into());
            args.push(project_dir.display().to_string());
        }
        for file in &target.compose_files {
            args.push("-f".into());
            args.push(file.display().to_string());
        }
        if let Some(project_name) = &target.compose_project {
            args.push("--project-name".into());
            args.push(project_name.clone());
        }
        match mutation {
            ComposeMutation::Up => {
                args.push("up".into());
                args.push("-d".into());
                args.push(target.target.service.clone());
            }
            ComposeMutation::Restart => {
                args.push("restart".into());
                args.push(target.target.service.clone());
            }
            ComposeMutation::Pull => {
                args.push("pull".into());
                args.push(target.target.service.clone());
            }
            ComposeMutation::Down => {
                args.push("down".into());
            }
        }
        ComposeInvocation {
            program: "docker".into(),
            args: {
                let mut all = vec!["compose".into()];
                all.extend(args);
                all
            },
            current_dir: target.compose_working_dir.clone(),
            timeout: self.defaults.timeout,
            output_limit_bytes: self.defaults.output_limit_bytes,
        }
    }
}
```

- [ ] **Step 3: Implement execution wrapper**

Append to `src/compose.rs`:

```rust
impl<I: DockerInspect, R: CommandRunner> ComposeService<I, R> {
    pub fn run_mutation(
        &self,
        mutation: ComposeMutation,
        requested: &ComposeTarget,
        options: &MutationOptions,
    ) -> Result<Option<CommandOutput>> {
        let target = self.resolve_target(requested)?;
        self.preflight_mutation(mutation, &target, options)?;
        if options.dry_run {
            return Ok(None);
        }
        let invocation = self.compose_invocation(&target, mutation);
        self.runner.run(&invocation).map(Some)
    }
}
```

- [ ] **Step 4: Add invocation tests**

Append to `src/compose_tests.rs`:

```rust
#[test]
fn up_invocation_is_detached_and_uses_project_directory_and_all_files() {
    let service = ComposeService::new(FakeInspector::default(), FakeRunner, ComposeDefaults::default());
    let target = ResolvedComposeTarget {
        target: ComposeTargetSummary {
            project_dir: Some(PathBuf::from("/tmp/project")),
            compose_file: Some(PathBuf::from("/tmp/project/base.yml")),
            project_name: Some("syslog-jmagar-lab".into()),
            service: "syslog-mcp".into(),
            container_name: "syslog-mcp".into(),
        },
        source: TargetSource::LiveContainerLabels,
        confidence: TargetConfidence::Confirmed,
        diagnostics: Vec::new(),
        compose_files: vec![
            PathBuf::from("/tmp/project/base.yml"),
            PathBuf::from("/tmp/project/override.yml"),
        ],
        compose_working_dir: Some(PathBuf::from("/tmp/project")),
        compose_project: Some("syslog-jmagar-lab".into()),
    };

    let invocation = service.compose_invocation(&target, ComposeMutation::Up);
    assert_eq!(invocation.program, "docker");
    assert_eq!(invocation.current_dir, Some(PathBuf::from("/tmp/project")));
    assert_eq!(
        invocation.args,
        vec![
            "compose",
            "--project-directory",
            "/tmp/project",
            "-f",
            "/tmp/project/base.yml",
            "-f",
            "/tmp/project/override.yml",
            "--project-name",
            "syslog-jmagar-lab",
            "up",
            "-d",
            "syslog-mcp",
        ]
    );
}

#[test]
fn dry_run_does_not_invoke_runner() {
    let service = ComposeService::new(
        FakeInspector {
            container: Some(labelled_container()),
            ..Default::default()
        },
        FakeRunner,
        ComposeDefaults::default(),
    );
    let output = service
        .run_mutation(
            ComposeMutation::Pull,
            &ComposeTarget::default(),
            &MutationOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(output.is_none());
}
```

- [ ] **Step 5: Add real runner implementation stub with explicit panic guard**

Add this near the bottom of `src/compose.rs`; it is a compiling stub that forces the later task to replace it before CLI use:

```rust
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, _invocation: &ComposeInvocation) -> Result<CommandOutput> {
        Err(anyhow!(
            "ProcessRunner is not implemented yet; finish bounded process execution before wiring CLI"
        ))
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test compose::tests --lib`

Expected: invocation and dry-run tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/compose.rs src/compose_tests.rs
git commit -m "feat: build compose command invocations"
```

## Task 4: Bounded Process Runner

**Files:**
- Modify: `src/compose.rs`
- Modify: `src/compose_tests.rs`

- [ ] **Step 1: Replace `ProcessRunner` with a bounded implementation**

Replace the `ProcessRunner` impl in `src/compose.rs` with:

```rust
impl CommandRunner for ProcessRunner {
    fn run(&self, invocation: &ComposeInvocation) -> Result<CommandOutput> {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex};
        use std::thread;
        use std::time::Instant;

        let mut command = Command::new(&invocation.program);
        command.args(&invocation.args);
        if let Some(dir) = &invocation.current_dir {
            command.current_dir(dir);
        }
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        let mut child = command.spawn().map_err(|e| {
            anyhow!(
                "failed to spawn {} {}: {e}",
                invocation.program,
                invocation.args.join(" ")
            )
        })?;

        let stdout = child.stdout.take().ok_or_else(|| anyhow!("missing stdout pipe"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("missing stderr pipe"))?;
        let stdout_buf = Arc::new(Mutex::new((Vec::new(), false)));
        let stderr_buf = Arc::new(Mutex::new((Vec::new(), false)));

        let out_handle = drain_pipe(stdout, Arc::clone(&stdout_buf), invocation.output_limit_bytes);
        let err_handle = drain_pipe(stderr, Arc::clone(&stderr_buf), invocation.output_limit_bytes);

        let started = Instant::now();
        let mut timed_out = false;
        let mut timeout_cleanup = None;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() >= invocation.timeout {
                timed_out = true;
                let terminate_sent = terminate_child(&mut child);
                std::thread::sleep(Duration::from_millis(500));
                let reaped_after_term = child.try_wait()?.is_some();
                let mut kill_sent = false;
                if !reaped_after_term {
                    kill_sent = child.kill().is_ok();
                }
                let reaped = child.wait().is_ok();
                timeout_cleanup = Some(TimeoutCleanupStatus {
                    terminate_sent,
                    kill_sent,
                    reaped,
                });
                break child.wait()?;
            }
            thread::sleep(Duration::from_millis(25));
        };

        let _ = out_handle.join();
        let _ = err_handle.join();

        let (stdout, stdout_truncated) = take_buffer(stdout_buf)?;
        let (stderr, stderr_truncated) = take_buffer(stderr_buf)?;

        Ok(CommandOutput {
            exit_status: status.code(),
            stdout: redact_sensitive(&String::from_utf8_lossy(&stdout)),
            stderr: redact_sensitive(&String::from_utf8_lossy(&stderr)),
            stdout_truncated,
            stderr_truncated,
            timed_out,
            timeout_cleanup,
        })
    }
}

fn drain_pipe<R: Read + Send + 'static>(
    mut reader: R,
    target: std::sync::Arc<std::sync::Mutex<(Vec<u8>, bool)>>,
    limit: usize,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let mut guard = target.lock().expect("pipe buffer mutex poisoned");
            let remaining = limit.saturating_sub(guard.0.len());
            if remaining > 0 {
                let keep = remaining.min(n);
                guard.0.extend_from_slice(&chunk[..keep]);
                if keep < n {
                    guard.1 = true;
                }
            } else {
                guard.1 = true;
            }
        }
    })
}

fn take_buffer(
    buffer: std::sync::Arc<std::sync::Mutex<(Vec<u8>, bool)>>,
) -> Result<(Vec<u8>, bool)> {
    let guard = buffer.lock().map_err(|_| anyhow!("pipe buffer mutex poisoned"))?;
    Ok((guard.0.clone(), guard.1))
}

#[cfg(unix)]
fn terminate_child(child: &mut std::process::Child) -> bool {
    let pid = child.id() as i32;
    unsafe { libc::kill(-pid, libc::SIGTERM) == 0 }
}

#[cfg(not(unix))]
fn terminate_child(child: &mut std::process::Child) -> bool {
    child.kill().is_ok()
}
```

- [ ] **Step 2: Add `libc` dependency if needed**

If `libc` is not already available transitively for direct use, add to `Cargo.toml`:

```toml
libc = "0.2"
```

Run: `cargo check`

Expected: if `libc` was missing before, direct use now compiles.

- [ ] **Step 3: Add bounded-output process tests**

Append to `src/compose_tests.rs`:

```rust
#[test]
fn process_runner_truncates_and_redacts_output() {
    let runner = ProcessRunner;
    let invocation = ComposeInvocation {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            "printf 'token=secret-value\\nvisible-line\\nmore-output\\n'".into(),
        ],
        current_dir: None,
        timeout: Duration::from_secs(5),
        output_limit_bytes: 32,
    };
    let output = runner.run(&invocation).unwrap();
    assert_eq!(output.exit_status, Some(0));
    assert!(output.stdout.contains("[REDACTED]"));
    assert!(!output.stdout.contains("secret-value"));
    assert!(output.stdout_truncated);
}

#[test]
fn process_runner_times_out_and_reports_cleanup() {
    let runner = ProcessRunner;
    let invocation = ComposeInvocation {
        program: "sh".into(),
        args: vec!["-c".into(), "sleep 5".into()],
        current_dir: None,
        timeout: Duration::from_millis(100),
        output_limit_bytes: 1024,
    };
    let output = runner.run(&invocation).unwrap();
    assert!(output.timed_out);
    assert!(output.timeout_cleanup.as_ref().is_some_and(|c| c.reaped));
}
```

- [ ] **Step 4: Run focused tests**

Run: `cargo test compose::tests --lib -- --test-threads=1`

Expected: process runner timeout test passes reliably.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/compose.rs src/compose_tests.rs
git commit -m "feat: add bounded compose process runner"
```

## Task 5: CLI Parsing and Runtime Split

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/cli_tests.rs`
- Modify: `src/main_tests.rs`

- [ ] **Step 1: Add compose CLI argument types**

In `src/cli.rs`, add to the command enums and structs near the top:

```rust
use syslog_mcp::compose::{
    ComposeDefaults, ComposeMutation, ComposeService, ComposeTarget, MutationOptions,
    ProcessRunner,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliCommand {
    Search(SearchArgs),
    Tail(TailArgs),
    Errors(TimeRangeArgs),
    Hosts(OutputArgs),
    Sessions(SessionsArgs),
    Ai(AiCommand),
    Correlate(CorrelateArgs),
    Stats(OutputArgs),
    Compose(ComposeCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposeCommand {
    Status(ComposeArgs),
    Doctor(ComposeArgs),
    Up(ComposeMutationArgs),
    Down(ComposeMutationArgs),
    Restart(ComposeMutationArgs),
    Pull(ComposeMutationArgs),
    Logs(ComposeLogsArgs),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ComposeArgs {
    pub target: ComposeTarget,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ComposeMutationArgs {
    pub target: ComposeTarget,
    pub options: MutationOptions,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ComposeLogsArgs {
    pub target: ComposeTarget,
    pub tail: Option<u32>,
    pub json: bool,
}
```

If the existing `CliCommand` enum already exists, add only the new `Compose` variant instead of duplicating the whole enum.

- [ ] **Step 2: Add parser dispatch**

In `impl CliCommand::parse`, add:

```rust
"compose" => parse_compose(rest),
```

Add parser helpers to `src/cli.rs`:

```rust
fn parse_compose(args: &[String]) -> Result<CliCommand> {
    let (subcommand, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("compose requires a subcommand"))?;
    match subcommand.as_str() {
        "status" => Ok(CliCommand::Compose(ComposeCommand::Status(parse_compose_args(rest)?))),
        "doctor" => Ok(CliCommand::Compose(ComposeCommand::Doctor(parse_compose_args(rest)?))),
        "up" => Ok(CliCommand::Compose(ComposeCommand::Up(parse_compose_mutation(rest, false)?))),
        "down" => Ok(CliCommand::Compose(ComposeCommand::Down(parse_compose_mutation(rest, true)?))),
        "restart" => Ok(CliCommand::Compose(ComposeCommand::Restart(parse_compose_mutation(rest, false)?))),
        "pull" => Ok(CliCommand::Compose(ComposeCommand::Pull(parse_compose_mutation(rest, false)?))),
        "logs" => Ok(CliCommand::Compose(ComposeCommand::Logs(parse_compose_logs(rest)?))),
        "config" => bail!("syslog compose config is deferred from the first pass"),
        "upgrade" => bail!("syslog compose upgrade is deferred; run `syslog compose pull` then `syslog compose up`"),
        other => bail!("unknown compose subcommand: {other}"),
    }
}

fn parse_compose_args(args: &[String]) -> Result<ComposeArgs> {
    let mut parsed = ComposeArgs::default();
    parse_compose_common(args, &mut parsed.target, &mut parsed.json)?;
    Ok(parsed)
}

fn parse_compose_mutation(args: &[String], destructive: bool) -> Result<ComposeMutationArgs> {
    let mut parsed = ComposeMutationArgs::default();
    parse_compose_common(args, &mut parsed.target, &mut parsed.json)?;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--dry-run" => parsed.options.dry_run = true,
            "--allow-cwd-target" => parsed.options.allow_cwd_target = true,
            "--allow-foreign-project" => parsed.options.allow_foreign_project = true,
            "--yes" => parsed.options.yes = true,
            _ if is_compose_common_arg(arg) => {
                if !arg.contains('=') && needs_value(arg) {
                    let _ = flags.value(arg)?;
                }
            }
            _ if arg.starts_with("--") => bail!("unknown compose option: {arg}"),
            _ => bail!("unexpected compose argument: {arg}"),
        }
    }
    parsed.options.non_interactive = destructive;
    Ok(parsed)
}

fn parse_compose_logs(args: &[String]) -> Result<ComposeLogsArgs> {
    let mut parsed = ComposeLogsArgs::default();
    parse_compose_common(args, &mut parsed.target, &mut parsed.json)?;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--tail" => parsed.tail = Some(parse_u32_flag("--tail", flags.value("--tail")?)?),
            _ if arg.starts_with("--tail=") => {
                parsed.tail = Some(parse_u32_flag("--tail", value_after_equals(arg, "--tail")?)?)
            }
            "--follow" => bail!("syslog compose logs --follow is deferred"),
            _ if is_compose_common_arg(arg) => {
                if !arg.contains('=') && needs_value(arg) {
                    let _ = flags.value(arg)?;
                }
            }
            _ if arg.starts_with("--") => bail!("unknown compose logs option: {arg}"),
            _ => bail!("unexpected compose logs argument: {arg}"),
        }
    }
    Ok(parsed)
}

fn parse_compose_common(args: &[String], target: &mut ComposeTarget, json: &mut bool) -> Result<()> {
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => *json = true,
            "--compose-file" => target.compose_file = Some(flags.value("--compose-file")?.into()),
            "--project-dir" => target.project_dir = Some(flags.value("--project-dir")?.into()),
            "--project-name" => target.project_name = Some(flags.value("--project-name")?),
            "--service" => target.service = Some(flags.value("--service")?),
            "--container" => target.container_name = Some(flags.value("--container")?),
            _ if arg.starts_with("--compose-file=") => {
                target.compose_file = Some(value_after_equals(arg, "--compose-file")?.into())
            }
            _ if arg.starts_with("--project-dir=") => {
                target.project_dir = Some(value_after_equals(arg, "--project-dir")?.into())
            }
            _ if arg.starts_with("--project-name=") => {
                target.project_name = Some(value_after_equals(arg, "--project-name")?)
            }
            _ if arg.starts_with("--service=") => {
                target.service = Some(value_after_equals(arg, "--service")?)
            }
            _ if arg.starts_with("--container=") => {
                target.container_name = Some(value_after_equals(arg, "--container")?)
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_compose_common_arg(arg: &str) -> bool {
    matches!(
        arg,
        "--json" | "--compose-file" | "--project-dir" | "--project-name" | "--service" | "--container"
    ) || arg.starts_with("--compose-file=")
        || arg.starts_with("--project-dir=")
        || arg.starts_with("--project-name=")
        || arg.starts_with("--service=")
        || arg.starts_with("--container=")
}

fn needs_value(arg: &str) -> bool {
    matches!(arg, "--compose-file" | "--project-dir" | "--project-name" | "--service" | "--container")
}
```

- [ ] **Step 3: Split CLI execution**

In `src/main.rs`, change `run_cli` so compose commands do not load the DB:

```rust
async fn run_cli(command: cli::CliCommand) -> Result<()> {
    if matches!(command, cli::CliCommand::Compose(_)) {
        return cli::run_compose(command);
    }
    let runtime = RuntimeCore::load_query_only().await?;
    cli::run(runtime.service(), command).await
}
```

Add `compose` to the top-level mode command list:

```rust
| "compose"
```

- [ ] **Step 4: Add CLI compose runner stub**

In `src/cli.rs`, add:

```rust
pub(crate) fn run_compose(command: CliCommand) -> Result<()> {
    let CliCommand::Compose(command) = command else {
        bail!("run_compose called with non-compose command");
    };
    let service = ComposeService::new(
        syslog_mcp::compose::CliDockerInspect::default(),
        ProcessRunner,
        ComposeDefaults::default(),
    );
    match command {
        ComposeCommand::Status(args) | ComposeCommand::Doctor(args) => {
            let status = service.status(&args.target)?;
            print_json_or_debug(&status, args.json)
        }
        ComposeCommand::Up(args) => print_json_or_debug(
            &service.run_mutation(ComposeMutation::Up, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Down(args) => print_json_or_debug(
            &service.run_mutation(ComposeMutation::Down, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Restart(args) => print_json_or_debug(
            &service.run_mutation(ComposeMutation::Restart, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Pull(args) => print_json_or_debug(
            &service.run_mutation(ComposeMutation::Pull, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Logs(args) => {
            bail!("compose logs implementation comes after bounded runner status wiring: tail={:?}", args.tail)
        }
    }
}

fn print_json_or_debug<T: serde::Serialize + std::fmt::Debug>(value: &T, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{value:#?}");
    }
    Ok(())
}
```

If `CliDockerInspect` is not implemented yet, temporarily keep `run_compose` returning a clear error and wire it in the Docker inspector task.

- [ ] **Step 5: Add parser tests**

Append to `src/cli_tests.rs`:

```rust
#[test]
fn parse_compose_status_collects_target() {
    let parsed = CliCommand::parse(strings(&[
        "compose",
        "status",
        "--compose-file",
        "/tmp/docker-compose.yml",
        "--project-name=syslog",
        "--json",
    ]))
    .unwrap();
    match parsed {
        CliCommand::Compose(ComposeCommand::Status(args)) => {
            assert_eq!(args.target.compose_file.unwrap(), std::path::PathBuf::from("/tmp/docker-compose.yml"));
            assert_eq!(args.target.project_name.as_deref(), Some("syslog"));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_compose_upgrade_is_deferred() {
    let err = CliCommand::parse(strings(&["compose", "upgrade"])).unwrap_err();
    assert!(err.to_string().contains("deferred"));
}

#[test]
fn parse_compose_logs_follow_is_deferred() {
    let err = CliCommand::parse(strings(&["compose", "logs", "--follow"])).unwrap_err();
    assert!(err.to_string().contains("deferred"));
}

#[test]
fn parse_compose_down_collects_yes_and_dry_run() {
    let parsed = CliCommand::parse(strings(&["compose", "down", "--yes", "--dry-run"])).unwrap();
    match parsed {
        CliCommand::Compose(ComposeCommand::Down(args)) => {
            assert!(args.options.yes);
            assert!(args.options.dry_run);
            assert!(args.options.non_interactive);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
```

- [ ] **Step 6: Add main parser test**

Append to `src/main_tests.rs`:

```rust
#[test]
fn mode_parse_accepts_compose_namespace() {
    assert!(matches!(
        Mode::parse(vec!["compose".into(), "status".into(), "--json".into()]).unwrap(),
        Mode::Cli(_)
    ));
}
```

- [ ] **Step 7: Run parser tests**

Run:

```bash
cargo test cli_tests main_tests --bin syslog
```

Expected: compose parser tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/cli.rs src/main.rs src/cli_tests.rs src/main_tests.rs
git commit -m "feat: parse compose lifecycle commands"
```

## Task 6: Docker CLI Inspector and Status/Doctor CLI

**Files:**
- Modify: `src/compose.rs`
- Modify: `src/compose_tests.rs`
- Modify: `src/cli.rs`

- [ ] **Step 1: Add `CliDockerInspect` shell implementation**

Append to `src/compose.rs`:

```rust
#[derive(Debug, Default, Clone, Copy)]
pub struct CliDockerInspect;

impl DockerInspect for CliDockerInspect {
    fn inspect_container(&self, name: &str) -> Result<Option<ContainerInfo>> {
        let output = std::process::Command::new("docker")
            .args(["inspect", name, "--format", "{{json .}}"])
            .output()?;
        if !output.status.success() {
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        container_info_from_inspect(value).map(Some)
    }

    fn find_candidates(&self, service: &str, container_name: &str) -> Result<Vec<ContainerInfo>> {
        let filter = format!("label=com.docker.compose.service={service}");
        let output = std::process::Command::new("docker")
            .args(["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"])
            .output()?;
        if !output.status.success() {
            return Ok(Vec::new());
        }
        let names = String::from_utf8_lossy(&output.stdout);
        let mut found = Vec::new();
        for name in names.lines().take(10) {
            if name == container_name || name.contains(service) {
                if let Some(info) = self.inspect_container(name)? {
                    found.push(info);
                }
            }
        }
        Ok(found)
    }

    fn systemd_status(&self, unit: &str) -> Result<Option<SystemdStatus>> {
        let output = std::process::Command::new("systemctl")
            .args(["--user", "is-active", unit])
            .output();
        match output {
            Ok(output) => Ok(Some(SystemdStatus {
                unit: unit.into(),
                active: output.status.success(),
            })),
            Err(_) => Ok(None),
        }
    }

    fn listeners(&self, ports: &[u16]) -> Result<Vec<ListenerInfo>> {
        let mut listeners = Vec::new();
        for port in ports {
            let port_arg = format!(":{port}");
            let output = std::process::Command::new("ss")
                .args(["-ltnup", "sport", "=", &port_arg])
                .output();
            if let Ok(output) = output {
                if output.status.success() && !output.stdout.is_empty() {
                    listeners.push(ListenerInfo {
                        port: *port,
                        process: Some(String::from_utf8_lossy(&output.stdout).to_string()),
                        belongs_to_target: false,
                    });
                }
            }
        }
        Ok(listeners)
    }
}
```

- [ ] **Step 2: Add JSON inspect conversion**

Append to `src/compose.rs`:

```rust
fn container_info_from_inspect(value: serde_json::Value) -> Result<ContainerInfo> {
    let labels = value
        .pointer("/Config/Labels")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let name = value
        .get("Name")
        .and_then(|v| v.as_str())
        .unwrap_or("syslog-mcp")
        .trim_start_matches('/')
        .to_string();
    let mounts = value
        .get("Mounts")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|m| MountInfo {
                    source: m.get("Source").and_then(|v| v.as_str()).map(PathBuf::from),
                    target: m
                        .get("Destination")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    kind: m.get("Type").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ContainerInfo {
        id: value.get("Id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        name,
        status: value.pointer("/State/Status").and_then(|v| v.as_str()).map(str::to_string),
        health: value
            .pointer("/State/Health/Status")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        image: value.pointer("/Config/Image").and_then(|v| v.as_str()).map(str::to_string),
        image_id: value.get("Image").and_then(|v| v.as_str()).map(str::to_string),
        labels,
        mounts,
        ports: Vec::new(),
    })
}
```

- [ ] **Step 3: Implement status**

Append to `src/compose.rs`:

```rust
impl<I: DockerInspect, R> ComposeService<I, R> {
    pub fn status(&self, requested: &ComposeTarget) -> Result<ComposeStatus> {
        let target = self.resolve_target(requested)?;
        let container_name = target.target.container_name.clone();
        let info = self.inspector.inspect_container(&container_name)?;
        let mut diagnostics = target.diagnostics.clone();
        if target.source == TargetSource::CurrentWorkingDirectory {
            diagnostics.push(ComposeDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "cwd_target".into(),
                message: "resolved from current working directory".into(),
            });
        }
        let systemd = self.inspector.systemd_status("syslog-mcp.service")?;
        if systemd.as_ref().is_some_and(|s| s.active) {
            diagnostics.push(ComposeDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "systemd_active".into(),
                message: "syslog-mcp.service is active".into(),
            });
        }
        Ok(ComposeStatus {
            container_name,
            container_id: info.as_ref().map(|i| i.id.clone()),
            status: info.as_ref().and_then(|i| i.status.clone()),
            health: info.as_ref().and_then(|i| i.health.clone()),
            image: info.as_ref().and_then(|i| i.image.clone()),
            image_id: info.as_ref().and_then(|i| i.image_id.clone()),
            compose_project: target.compose_project,
            compose_working_dir: target.compose_working_dir,
            compose_files: target.compose_files,
            service: Some(target.target.service),
            data_mounts: info.as_ref().map(|i| i.mounts.clone()).unwrap_or_default(),
            ports: info.as_ref().map(|i| i.ports.clone()).unwrap_or_default(),
            systemd,
            diagnostics,
        })
    }
}
```

- [ ] **Step 4: Replace CLI status/doctor output**

In `src/cli.rs`, update `run_compose` status/doctor branches to call `status` and print `ComposeStatus`.

For human output, add:

```rust
fn print_compose_status(status: &syslog_mcp::compose::ComposeStatus) {
    println!("Container: {}", status.container_name);
    if let Some(value) = &status.status {
        println!("Status: {value}");
    }
    if let Some(value) = &status.health {
        println!("Docker health: {value}");
    }
    if let Some(value) = &status.image {
        println!("Image: {value}");
    }
    if let Some(value) = &status.compose_project {
        println!("Compose project: {value}");
    }
    if let Some(value) = &status.compose_working_dir {
        println!("Compose working dir: {}", value.display());
    }
    for diag in &status.diagnostics {
        println!("{:?}: {} - {}", diag.severity, diag.code, diag.message);
    }
}
```

- [ ] **Step 5: Run status dry check locally**

Run:

```bash
cargo run -- compose status --json
```

Expected: JSON prints, or a clear Docker/target diagnostic if Docker is unavailable. It must not attempt to open the SQLite DB.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test compose::tests cli_tests main_tests --lib --bin syslog
```

Expected: tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/compose.rs src/compose_tests.rs src/cli.rs
git commit -m "feat: add compose status diagnostics"
```

## Task 7: MCP Read-Only Compose Diagnostics

**Files:**
- Modify: `src/mcp/schemas.rs`
- Modify: `src/mcp/tools.rs`
- Modify: `src/mcp/rmcp_server.rs`
- Modify: `src/mcp/tools_tests.rs`
- Modify: `src/mcp/rmcp_server_tests.rs`

- [ ] **Step 1: Add schema actions**

In `src/mcp/schemas.rs`, add to `SYSLOG_ACTIONS`:

```rust
"compose_status",
"compose_doctor",
```

Update the tool description string to include `syslog compose_status` and `syslog compose_doctor`.

- [ ] **Step 2: Add read-scope entries**

In `src/mcp/rmcp_server.rs`, add both actions to `READ_ONLY_ACTIONS`:

```rust
"compose_status",
"compose_doctor",
```

- [ ] **Step 3: Add MCP target override rejection**

In `src/mcp/tools.rs`, add:

```rust
fn reject_compose_target_overrides(args: &Value) -> anyhow::Result<()> {
    for key in ["container", "container_name", "project_dir", "compose_file", "project_name"] {
        if args.get(key).is_some() {
            anyhow::bail!("compose MCP actions do not accept target override: {key}");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Dispatch compose actions**

In `tool_syslog`, add:

```rust
"compose_status" => tool_compose_status(args).await,
"compose_doctor" => tool_compose_status(args).await,
```

Add function:

```rust
async fn tool_compose_status(args: Value) -> anyhow::Result<Value> {
    reject_compose_target_overrides(&args)?;
    let service = crate::compose::ComposeService::new(
        crate::compose::CliDockerInspect::default(),
        crate::compose::ProcessRunner,
        crate::compose::ComposeDefaults::default(),
    );
    let status = tokio::task::spawn_blocking(move || {
        service.status(&crate::compose::ComposeTarget::default())
    })
    .await
    .map_err(|e| anyhow::anyhow!("compose status task failed: {e}"))??;
    Ok(serde_json::to_value(crate::compose::mcp_projection(&status))?)
}
```

- [ ] **Step 5: Add MCP tests**

Append to `src/mcp/tools_tests.rs`:

```rust
#[tokio::test]
async fn compose_action_rejects_target_override() {
    let h = TestHarness::new();
    let err = execute_tool(
        &h.state,
        "syslog",
        json!({"action": "compose_status", "project_dir": "/home/jmagar"}),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("target override"));
}
```

Append to `src/mcp/rmcp_server_tests.rs`:

```rust
#[test]
fn compose_actions_require_read_scope() {
    assert_eq!(required_scope_for("compose_status"), Some("syslog:read"));
    assert_eq!(required_scope_for("compose_doctor"), Some("syslog:read"));
}
```

- [ ] **Step 6: Run MCP tests**

Run:

```bash
cargo test mcp::tools_tests mcp::rmcp_server_tests --lib
```

Expected: compose schema/scope tests pass. If live Docker is unavailable in unit tests, keep the override rejection test only and add a fake compose service seam before testing successful status.

- [ ] **Step 7: Commit**

```bash
git add src/mcp/schemas.rs src/mcp/tools.rs src/mcp/rmcp_server.rs src/mcp/tools_tests.rs src/mcp/rmcp_server_tests.rs
git commit -m "feat: expose read-only compose diagnostics over mcp"
```

## Task 8: Bounded Logs and Mutation CLI Execution

**Files:**
- Modify: `src/compose.rs`
- Modify: `src/cli.rs`
- Modify: `src/compose_tests.rs`

- [ ] **Step 1: Add bounded logs invocation**

Add to `src/compose.rs`:

```rust
impl<I, R> ComposeService<I, R> {
    pub fn logs_invocation(&self, target: &ResolvedComposeTarget, tail: u32) -> ComposeInvocation {
        let mut invocation = self.compose_invocation(target, ComposeMutation::Pull);
        invocation.args.truncate(invocation.args.len().saturating_sub(2));
        invocation.args.push("logs".into());
        invocation.args.push("--tail".into());
        invocation.args.push(tail.to_string());
        invocation.args.push(target.target.service.clone());
        invocation
    }
}

impl<I: DockerInspect, R: CommandRunner> ComposeService<I, R> {
    pub fn logs(&self, requested: &ComposeTarget, tail: Option<u32>) -> Result<CommandOutput> {
        let target = self.resolve_target(requested)?;
        let invocation = self.logs_invocation(&target, tail.unwrap_or(100));
        self.runner.run(&invocation)
    }
}
```

- [ ] **Step 2: Wire CLI mutation branches**

In `src/cli.rs`, finish `run_compose`:

```rust
ComposeCommand::Logs(args) => {
    let output = service.logs(&args.target, args.tail)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print!("{}", output.stdout);
        eprint!("{}", output.stderr);
    }
    Ok(())
}
```

For mutation output, print command result and exit nonzero when `exit_status != Some(0)`:

```rust
fn ensure_command_success(output: &syslog_mcp::compose::CommandOutput) -> Result<()> {
    if output.exit_status == Some(0) && !output.timed_out {
        return Ok(());
    }
    anyhow::bail!(
        "compose command failed: status={:?} timed_out={} stderr={}",
        output.exit_status,
        output.timed_out,
        output.stderr
    )
}
```

- [ ] **Step 3: Add logs invocation test**

Append to `src/compose_tests.rs`:

```rust
#[test]
fn logs_invocation_is_bounded_tail() {
    let service = ComposeService::new(FakeInspector::default(), FakeRunner, ComposeDefaults::default());
    let target = target_from_container(&labelled_container(), &ComposeDefaults::default());
    let invocation = service.logs_invocation(&target, 20);
    assert!(invocation.args.ends_with(&[
        "logs".into(),
        "--tail".into(),
        "20".into(),
        "syslog-mcp".into(),
    ]));
}
```

- [ ] **Step 4: Run dry-run live commands**

Run:

```bash
cargo run -- compose pull --dry-run --json
cargo run -- compose up --dry-run --json
```

Expected: commands resolve/preflight and do not mutate Docker.

- [ ] **Step 5: Run bounded logs**

Run:

```bash
cargo run -- compose logs --tail 20
```

Expected: prints at most the bounded Compose logs command output, or a clear Docker/target diagnostic.

- [ ] **Step 6: Commit**

```bash
git add src/compose.rs src/compose_tests.rs src/cli.rs
git commit -m "feat: run compose lifecycle commands"
```

## Task 9: Documentation and Final Verification

**Files:**
- Modify: `README.md`
- Modify: `docs/mcp/DEPLOY.md`
- Modify: `docs/runbooks/deploy.md`
- Modify: `AGENTS.md` if present and command lists are stale

- [ ] **Step 1: Update README command list**

Add these commands near the existing CLI command section:

```bash
syslog compose doctor          # diagnose live Compose/systemd/listener ownership
syslog compose status --json   # inspect canonical syslog-mcp container/project
syslog compose pull            # pull image for resolved Compose project
syslog compose up              # run docker compose up -d for resolved service
syslog compose restart         # restart resolved service
syslog compose logs --tail 20  # bounded compose logs
```

Add the note:

```markdown
`syslog compose` commands resolve the live Compose owner before mutation. They refuse ambiguous cwd fallback, stale Compose labels, systemd/listener conflicts, and destructive `down` without `--yes`.
```

- [ ] **Step 2: Update MCP deploy docs**

In `docs/mcp/DEPLOY.md`, add:

```markdown
MCP exposes only redacted read-only Compose diagnostics (`compose_status`, `compose_doctor`). Lifecycle mutations remain CLI-only: ask the assistant to run `syslog compose ...` locally rather than invoking MCP actions.
```

- [ ] **Step 3: Update runbook**

In `docs/runbooks/deploy.md`, replace raw Compose lifecycle examples with:

```bash
syslog compose doctor
syslog compose status
syslog compose pull
syslog compose up
syslog compose logs --tail 50
```

- [ ] **Step 4: Full verification**

Run:

```bash
cargo fmt
cargo test
cargo clippy -- -D warnings
cargo run -- compose status --json
cargo run -- compose doctor
cargo run -- compose logs --tail 20
```

Expected: format succeeds, tests pass, clippy is clean, and compose commands either work against Docker or produce clear diagnostics without loading the SQLite query runtime.

- [ ] **Step 5: Version bump if this is a feature branch push**

If preparing to push this branch, run the repo’s version-bump workflow because this repo requires version bumps on feature branch pushes:

```bash
./scripts/bump-version.sh minor
./scripts/check-version-sync.sh
```

Expected: all version-bearing files agree and `CHANGELOG.md` has a new entry.

- [ ] **Step 6: Commit docs and verification fixes**

```bash
git add README.md docs/mcp/DEPLOY.md docs/runbooks/deploy.md AGENTS.md Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "docs: document compose lifecycle commands"
```

## Self-Review

- Spec coverage: the plan covers shared compose layer, CLI full lifecycle, MCP read-only diagnostics, no DB runtime load, mutation safety, subprocess bounds, process cleanup, concurrent pipe draining, Compose cwd semantics, systemd/listener checks, and documentation.
- Placeholder scan: there are no `TBD`, `TODO`, or “implement later” instructions. Deferred features are explicitly excluded from the first pass and named in CLI/MCP rejection behavior.
- Type consistency: `ComposeStatus` is local/CLI, `ComposeMcpStatus` is MCP-safe, `ComposeTarget` is input, `ResolvedComposeTarget` is resolution output, `CommandRunner` returns `CommandOutput`, and CLI parser variants map to `ComposeCommand`.
