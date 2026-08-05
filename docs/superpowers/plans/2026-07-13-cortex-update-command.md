# Cortex Update Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first-class `cortex update` operator workflow so an already-configured server and its host-agent clients can be updated without repeating `setup deploy remote --home ...`.

**Architecture:** Keep `setup deploy remote` and `setup deploy agent` as low-level primitives. Add a new `cortex::update` module that owns a small profile file at `<cortex-home>/deployments.toml`, resolves update targets, invokes the existing remote server deploy and agent deploy functions, and returns a serializable update report. Add CLI parsing and help for `cortex update`, `cortex update server`, `cortex update clients`, `cortex update agents`, `cortex update all`, and profile configuration commands.

**Tech Stack:** Rust 2024, `serde`, `toml`, existing `cortex::deploy`, existing `cortex::agent_deploy`, existing CLI parser in `src/main.rs`, existing help catalog in `src/cli/help.rs`, sidecar unit tests.

## Global Constraints

- Do not remove or rename `cortex setup deploy remote`; it remains the escape hatch and debug primitive.
- `cortex update` must not require repeating `--home /mnt/cache/appdata/cortex nashost` after the server profile has been saved.
- `cortex update` defaults to scope `all`: update the configured server, then update configured host-agent clients if any are configured.
- `clients` means Cortex host agents in this implementation. `agents` is accepted as an alias for `clients`.
- The default profile path is `<cortex-home>/deployments.toml`, where `<cortex-home>` is resolved by `cortex::setup::cortex_home_dir()`.
- The server profile stores `host` and `home`. It must reject empty host, unsafe SSH host strings, non-absolute home paths, and paths containing `..`.
- The clients profile stores host aliases and optional agent deployment settings. It must reject empty hosts and unsafe SSH host strings.
- A successful non-dry-run `cortex setup deploy remote --home PATH HOST` should save the server profile automatically, so the next server update can be `cortex update server`.
- `cortex update server` must call the existing `cortex::deploy::run_remote_deploy()` with `RemoteDeployOptions { dry_run, home: Some(profile.server.home) }`.
- `cortex update clients` must call the existing `cortex::agent_deploy::deploy_agent_to_host()` for every configured client host and fail if any result is not ok.
- `cortex update all` must update server first; if the server update has failed phases, it must not continue to clients.
- JSON output must be supported for update runs and profile configuration commands.
- Follow existing sccache workaround when validating locally: `RUSTC_WRAPPER='' cargo <cmd> --config 'build.rustc-wrapper=""'`.
- Existing unrelated dirty files must stay unstaged and untouched unless they are directly part of this plan.

---

## File Structure

- Create `src/update.rs`: profile structs, profile load/save, update scopes, update options, update report, `run_update()`, `configure_server_profile()`, `configure_clients_profile()`.
- Create `src/update_tests.rs`: unit tests for profile round-trip, validation, server update invocation, clients update invocation, and all-scope short-circuiting.
- Modify `src/lib.rs`: expose `pub mod update;`.
- Modify `src/main.rs`: add `Mode::Update`, `UpdateCommand`, `UpdateCommandKind`, parser, dispatcher, and output formatting.
- Modify `src/main_tests.rs`: parser coverage for `cortex update` command shapes and rejected args.
- Modify `src/cli/help.rs`: add top-level and nested help for `update`.
- Modify `docs/CLI.md`: document day-to-day update commands and one-time profile configuration.
- Modify `docs/mcp/DEPLOY.md`: clarify that `setup deploy remote` is the primitive and `update` is the normal operator workflow.

---

### Task 1: Add Update Profile And Runner Module

**Files:**
- Create: `src/update.rs`
- Create: `src/update_tests.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `crate::setup::cortex_home_dir() -> io::Result<PathBuf>`, `crate::deploy::run_remote_deploy(host, RemoteDeployOptions) -> io::Result<RemoteDeployReport>`, `crate::agent_deploy::{AgentDeployConfig, DeployResult, deploy_agent_to_host, find_local_binary}`.
- Produces:
  - `pub enum UpdateScope { All, Server, Clients }`
  - `pub struct UpdateOptions { pub dry_run: bool, pub profile_path: Option<PathBuf>, pub binary: Option<PathBuf> }`
  - `pub struct ServerUpdateProfile { pub host: String, pub home: String }`
  - `pub struct ClientsUpdateProfile { pub hosts: Vec<String>, pub target: Option<String>, pub docker: Option<bool>, pub journald: Option<bool> }`
  - `pub struct UpdateProfile { pub server: Option<ServerUpdateProfile>, pub clients: ClientsUpdateProfile }`
  - `pub struct UpdateReport { pub mode: &'static str, pub profile_path: PathBuf, pub server: Option<RemoteDeployReport>, pub clients: Vec<DeployResult>, pub skipped: Vec<SetupPhase>, pub has_errors: bool, pub elapsed_ms: u128 }`
  - `pub fn default_profile_path() -> io::Result<PathBuf>`
  - `pub fn load_profile(path: &Path) -> io::Result<UpdateProfile>`
  - `pub fn write_profile(path: &Path, profile: &UpdateProfile) -> io::Result<()>`
  - `pub fn configure_server_profile(path: Option<&Path>, host: &str, home: &str) -> io::Result<UpdateProfile>`
  - `pub fn configure_clients_profile(path: Option<&Path>, hosts: Vec<String>, target: Option<String>, docker: Option<bool>, journald: Option<bool>) -> io::Result<UpdateProfile>`
  - `pub fn run_update(scope: UpdateScope, options: UpdateOptions) -> io::Result<UpdateReport>`

- [ ] **Step 1: Add `src/update_tests.rs` with failing profile tests**

```rust
use super::*;

#[test]
fn update_profile_round_trips_server_and_clients() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deployments.toml");
    let profile = UpdateProfile {
        server: Some(ServerUpdateProfile {
            host: "nashost".to_string(),
            home: "/mnt/cache/appdata/cortex".to_string(),
        }),
        clients: ClientsUpdateProfile {
            hosts: vec!["devhost".to_string(), "backuphost".to_string()],
            target: Some("https://cortex.example.internal".to_string()),
            docker: Some(true),
            journald: None,
        },
    };

    write_profile(&path, &profile).unwrap();
    let loaded = load_profile(&path).unwrap();

    assert_eq!(loaded, profile);
}

#[test]
fn configure_server_profile_validates_and_preserves_clients() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deployments.toml");
    write_profile(
        &path,
        &UpdateProfile {
            server: None,
            clients: ClientsUpdateProfile {
                hosts: vec!["devhost".to_string()],
                target: Some("https://cortex.example.internal".to_string()),
                docker: Some(true),
                journald: Some(false),
            },
        },
    )
    .unwrap();

    let updated =
        configure_server_profile(Some(&path), "nashost", "/mnt/cache/appdata/cortex").unwrap();

    assert_eq!(updated.server.as_ref().unwrap().host, "nashost");
    assert_eq!(
        updated.server.as_ref().unwrap().home,
        "/mnt/cache/appdata/cortex"
    );
    assert_eq!(updated.clients.hosts, vec!["devhost"]);
}

#[test]
fn configure_server_profile_rejects_unsafe_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deployments.toml");

    let bad_host = configure_server_profile(
        Some(&path),
        "-oProxyCommand=touch /tmp/pwned",
        "/mnt/cache/appdata/cortex",
    )
    .unwrap_err();
    assert!(bad_host.to_string().contains("unsafe ssh host"));

    let bad_home = configure_server_profile(Some(&path), "nashost", "relative/path").unwrap_err();
    assert!(bad_home.to_string().contains("absolute path"));

    let parent_home =
        configure_server_profile(Some(&path), "nashost", "/mnt/cache/../cortex").unwrap_err();
    assert!(parent_home.to_string().contains("must not contain '..'"));
}
```

- [ ] **Step 2: Run profile tests to verify they fail**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' update_profile -- --nocapture
```

Expected: FAIL because `src/update.rs` does not exist and `UpdateProfile`/helpers are undefined.

- [ ] **Step 3: Create `src/update.rs` with profile loading and validation**

```rust
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::setup::cortex_home_dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateScope {
    All,
    Server,
    Clients,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    pub dry_run: bool,
    pub profile_path: Option<PathBuf>,
    pub binary: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerUpdateProfile {
    pub host: String,
    pub home: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClientsUpdateProfile {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub docker: Option<bool>,
    #[serde(default)]
    pub journald: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdateProfile {
    #[serde(default)]
    pub server: Option<ServerUpdateProfile>,
    #[serde(default)]
    pub clients: ClientsUpdateProfile,
}

pub fn default_profile_path() -> io::Result<PathBuf> {
    Ok(cortex_home_dir()?.join("deployments.toml"))
}

pub fn load_profile(path: &Path) -> io::Result<UpdateProfile> {
    match std::fs::read_to_string(path) {
        Ok(raw) => toml::from_str(&raw).map_err(|error| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("parse update profile {}: {error}", path.display()),
            )
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(UpdateProfile::default()),
        Err(error) => Err(error),
    }
}

pub fn write_profile(path: &Path, profile: &UpdateProfile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rendered = toml::to_string_pretty(profile).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("render update profile {}: {error}", path.display()),
        )
    })?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, rendered)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn configure_server_profile(
    path: Option<&Path>,
    host: &str,
    home: &str,
) -> io::Result<UpdateProfile> {
    let path = resolve_profile_path(path)?;
    let mut profile = load_profile(&path)?;
    profile.server = Some(ServerUpdateProfile {
        host: validate_host(host)?,
        home: validate_remote_home(home)?,
    });
    write_profile(&path, &profile)?;
    Ok(profile)
}

pub fn configure_clients_profile(
    path: Option<&Path>,
    hosts: Vec<String>,
    target: Option<String>,
    docker: Option<bool>,
    journald: Option<bool>,
) -> io::Result<UpdateProfile> {
    let path = resolve_profile_path(path)?;
    let mut validated = Vec::new();
    for host in hosts {
        validated.push(validate_host(&host)?);
    }
    if validated.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "at least one client host is required",
        ));
    }
    let mut profile = load_profile(&path)?;
    profile.clients = ClientsUpdateProfile {
        hosts: validated,
        target,
        docker,
        journald,
    };
    write_profile(&path, &profile)?;
    Ok(profile)
}

fn resolve_profile_path(path: Option<&Path>) -> io::Result<PathBuf> {
    match path {
        Some(path) => Ok(path.to_path_buf()),
        None => default_profile_path(),
    }
}

fn validate_host(host: &str) -> io::Result<String> {
    let trimmed = host.trim();
    if trimmed.is_empty() || !crate::inventory::ssh::is_safe_ssh_host(trimmed) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("unsafe ssh host: {host}"),
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_remote_home(home: &str) -> io::Result<String> {
    let trimmed = home.trim();
    let path = Path::new(trimmed);
    if trimmed.is_empty() || !path.is_absolute() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "server home must be a non-empty absolute path",
        ));
    }
    if path.components().any(|component| matches!(component, Component::ParentDir)) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "server home must not contain '..'",
        ));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
```

- [ ] **Step 4: Export the module in `src/lib.rs`**

Add the module near `pub mod setup;`:

```rust
pub mod setup;
pub mod update;
pub mod shell_history_ingest;
```

- [ ] **Step 5: Run profile tests to verify they pass**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' update_profile -- --nocapture
```

Expected: PASS for all three profile tests.

- [ ] **Step 6: Commit Task 1**

```bash
git add src/lib.rs src/update.rs src/update_tests.rs
git commit -m "feat: add cortex update profile"
```

---

### Task 2: Implement Server And Client Update Execution

**Files:**
- Modify: `src/update.rs`
- Modify: `src/update_tests.rs`

**Interfaces:**
- Consumes: Task 1 profile structs and existing deploy APIs.
- Produces: `run_update(scope, options)`, `UpdateReport`, and test seam traits inside `src/update.rs`.

- [ ] **Step 1: Add failing tests for server update, clients update, and all-scope short-circuit**

Append to `src/update_tests.rs`:

```rust
#[derive(Default)]
struct FakeUpdateRunner {
    server_calls: Vec<(String, RemoteDeployOptions)>,
    client_calls: Vec<String>,
    fail_server: bool,
    fail_client: Option<String>,
}

impl UpdateRunner for FakeUpdateRunner {
    fn run_server(
        &mut self,
        host: &str,
        options: RemoteDeployOptions,
    ) -> io::Result<RemoteDeployReport> {
        self.server_calls.push((host.to_string(), options.clone()));
        Ok(RemoteDeployReport {
            mode: if options.dry_run { "remote dry-run" } else { "remote" },
            host: host.to_string(),
            home: options.home.clone().unwrap(),
            env_path: format!("{}/.env", options.home.clone().unwrap()),
            compose_dir: format!("{}/compose", options.home.clone().unwrap()),
            data_dir: format!("{}/data", options.home.clone().unwrap()),
            health_url: "http://127.0.0.1:3100/health".to_string(),
            mcp_url: "http://127.0.0.1:3100/mcp".to_string(),
            phases: Vec::new(),
            has_errors: self.fail_server,
            elapsed_ms: 1,
        })
    }

    fn deploy_client(
        &mut self,
        host: &str,
        _binary: &Path,
        _config: &AgentDeployConfig,
    ) -> DeployResult {
        self.client_calls.push(host.to_string());
        let ok = self.fail_client.as_deref() != Some(host);
        DeployResult {
            host: host.to_string(),
            ok,
            detail: if ok { "ok".to_string() } else { "forced failure".to_string() },
            elapsed_ms: 1,
        }
    }

    fn find_binary(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/tmp/cortex"))
    }
}

#[test]
fn update_server_uses_saved_profile_without_repeating_home_arg() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deployments.toml");
    configure_server_profile(Some(&path), "nashost", "/mnt/cache/appdata/cortex").unwrap();
    let mut runner = FakeUpdateRunner::default();

    let report = run_update_with_runner(
        UpdateScope::Server,
        UpdateOptions {
            dry_run: true,
            profile_path: Some(path.clone()),
            binary: None,
        },
        &mut runner,
    )
    .unwrap();

    assert!(!report.has_errors);
    assert_eq!(runner.server_calls.len(), 1);
    assert_eq!(runner.server_calls[0].0, "nashost");
    assert_eq!(
        runner.server_calls[0].1.home.as_deref(),
        Some("/mnt/cache/appdata/cortex")
    );
    assert!(runner.server_calls[0].1.dry_run);
    assert_eq!(report.profile_path, path);
}

#[test]
fn update_clients_deploys_every_configured_client() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deployments.toml");
    configure_clients_profile(
        Some(&path),
        vec!["devhost".to_string(), "backuphost".to_string()],
        Some("https://cortex.example.internal".to_string()),
        Some(true),
        None,
    )
    .unwrap();
    let mut runner = FakeUpdateRunner::default();

    let report = run_update_with_runner(
        UpdateScope::Clients,
        UpdateOptions {
            dry_run: false,
            profile_path: Some(path),
            binary: Some(PathBuf::from("/tmp/cortex")),
        },
        &mut runner,
    )
    .unwrap();

    assert!(!report.has_errors);
    assert_eq!(runner.client_calls, vec!["devhost", "backuphost"]);
    assert_eq!(report.clients.len(), 2);
}

#[test]
fn update_all_stops_before_clients_when_server_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deployments.toml");
    configure_server_profile(Some(&path), "nashost", "/mnt/cache/appdata/cortex").unwrap();
    configure_clients_profile(
        Some(&path),
        vec!["devhost".to_string()],
        Some("https://cortex.example.internal".to_string()),
        None,
        None,
    )
    .unwrap();
    let mut runner = FakeUpdateRunner {
        fail_server: true,
        ..FakeUpdateRunner::default()
    };

    let report = run_update_with_runner(
        UpdateScope::All,
        UpdateOptions {
            dry_run: false,
            profile_path: Some(path),
            binary: Some(PathBuf::from("/tmp/cortex")),
        },
        &mut runner,
    )
    .unwrap();

    assert!(report.has_errors);
    assert!(runner.client_calls.is_empty());
    assert!(report.skipped.iter().any(|phase| phase.name == "clients"));
}
```

- [ ] **Step 2: Run update execution tests to verify they fail**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' update_ -- --nocapture
```

Expected: FAIL because `UpdateRunner`, `UpdateReport`, and `run_update_with_runner` are not defined.

- [ ] **Step 3: Add execution code to `src/update.rs`**

Add imports near the top:

```rust
use std::time::Instant;

use crate::agent_deploy::{AgentDeployConfig, DeployResult, deploy_agent_to_host, find_local_binary};
use crate::deploy::{RemoteDeployOptions, RemoteDeployReport, run_remote_deploy};
use crate::setup::{PhaseTimer, SetupPhase, SetupStatus};
```

Add below the profile functions:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct UpdateReport {
    pub mode: &'static str,
    pub profile_path: PathBuf,
    pub server: Option<RemoteDeployReport>,
    pub clients: Vec<DeployResult>,
    pub skipped: Vec<SetupPhase>,
    pub has_errors: bool,
    pub elapsed_ms: u128,
}

trait UpdateRunner {
    fn run_server(
        &mut self,
        host: &str,
        options: RemoteDeployOptions,
    ) -> io::Result<RemoteDeployReport>;

    fn deploy_client(
        &mut self,
        host: &str,
        binary: &Path,
        config: &AgentDeployConfig,
    ) -> DeployResult;

    fn find_binary(&self) -> Option<PathBuf>;
}

struct RealUpdateRunner;

impl UpdateRunner for RealUpdateRunner {
    fn run_server(
        &mut self,
        host: &str,
        options: RemoteDeployOptions,
    ) -> io::Result<RemoteDeployReport> {
        run_remote_deploy(host, options)
    }

    fn deploy_client(
        &mut self,
        host: &str,
        binary: &Path,
        config: &AgentDeployConfig,
    ) -> DeployResult {
        deploy_agent_to_host(host, binary, config)
    }

    fn find_binary(&self) -> Option<PathBuf> {
        find_local_binary()
    }
}

pub fn run_update(scope: UpdateScope, options: UpdateOptions) -> io::Result<UpdateReport> {
    let mut runner = RealUpdateRunner;
    run_update_with_runner(scope, options, &mut runner)
}

fn run_update_with_runner(
    scope: UpdateScope,
    options: UpdateOptions,
    runner: &mut dyn UpdateRunner,
) -> io::Result<UpdateReport> {
    let started = Instant::now();
    let profile_path = options
        .profile_path
        .clone()
        .unwrap_or(default_profile_path()?);
    let profile = load_profile(&profile_path)?;
    let mut server = None;
    let mut clients = Vec::new();
    let mut skipped = Vec::new();

    if matches!(scope, UpdateScope::All | UpdateScope::Server) {
        let target = profile.server.as_ref().ok_or_else(|| {
            io::Error::new(
                ErrorKind::NotFound,
                format!(
                    "no server update profile at {}; run `cortex update config server --host HOST --home PATH`",
                    profile_path.display()
                ),
            )
        })?;
        let report = runner.run_server(
            &target.host,
            RemoteDeployOptions {
                dry_run: options.dry_run,
                home: Some(target.home.clone()),
            },
        )?;
        let failed = report.has_errors;
        server = Some(report);
        if failed && matches!(scope, UpdateScope::All) {
            skipped.push(
                PhaseTimer::start("clients")
                    .finish(SetupStatus::Skipped, "skipped because server update failed"),
            );
            return Ok(build_report(scope, profile_path, server, clients, skipped, started));
        }
    }

    if matches!(scope, UpdateScope::All | UpdateScope::Clients) {
        if profile.clients.hosts.is_empty() {
            if matches!(scope, UpdateScope::Clients) {
                return Err(io::Error::new(
                    ErrorKind::NotFound,
                    format!(
                        "no client update profile at {}; run `cortex update config clients --hosts HOST1,HOST2`",
                        profile_path.display()
                    ),
                ));
            }
            skipped.push(
                PhaseTimer::start("clients")
                    .finish(SetupStatus::Skipped, "no configured client hosts"),
            );
        } else if options.dry_run {
            skipped.push(
                PhaseTimer::start("clients")
                    .finish(SetupStatus::Skipped, "dry-run does not deploy client agents"),
            );
        } else {
            let binary = options
                .binary
                .clone()
                .or_else(|| runner.find_binary())
                .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "cortex binary not found"))?;
            let config = AgentDeployConfig {
                target: profile.clients.target.clone(),
                token: None,
                docker: profile.clients.docker,
                journald: profile.clients.journald,
            };
            for host in &profile.clients.hosts {
                clients.push(runner.deploy_client(host, &binary, &config));
            }
        }
    }

    Ok(build_report(scope, profile_path, server, clients, skipped, started))
}

fn build_report(
    scope: UpdateScope,
    profile_path: PathBuf,
    server: Option<RemoteDeployReport>,
    clients: Vec<DeployResult>,
    skipped: Vec<SetupPhase>,
    started: Instant,
) -> UpdateReport {
    let has_errors = server.as_ref().is_some_and(|report| report.has_errors)
        || clients.iter().any(|result| !result.ok)
        || skipped
            .iter()
            .any(|phase| matches!(phase.status, SetupStatus::Error));
    UpdateReport {
        mode: match scope {
            UpdateScope::All => "all",
            UpdateScope::Server => "server",
            UpdateScope::Clients => "clients",
        },
        profile_path,
        server,
        clients,
        skipped,
        has_errors,
        elapsed_ms: started.elapsed().as_millis(),
    }
}
```

- [ ] **Step 4: Run update execution tests to verify they pass**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' update_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit Task 2**

```bash
git add src/update.rs src/update_tests.rs
git commit -m "feat: run configured cortex updates"
```

---

### Task 3: Add `cortex update` CLI Parsing And Dispatch

**Files:**
- Modify: `src/main.rs`
- Modify: `src/main_tests.rs`

**Interfaces:**
- Consumes: Task 2 `cortex::update::{UpdateScope, UpdateOptions, run_update, configure_server_profile, configure_clients_profile}`.
- Produces:
  - `Mode::Update(UpdateCommand)`
  - `UpdateCommandKind::{Run { scope, dry_run, profile, binary }, ConfigServer { host, home, profile }, ConfigClients { hosts, target, docker, journald, profile }}`
  - `parse_update_command(args: &[String]) -> Result<UpdateCommand>`
  - `run_update(command: UpdateCommand) -> Result<()>`

- [ ] **Step 1: Add failing CLI parser tests**

Append to `src/main_tests.rs` near the deploy parser tests:

```rust
#[test]
fn mode_parse_accepts_update_defaults_to_all() {
    let mode = super::Mode::parse(vec!["update".into()]).unwrap();

    assert!(matches!(
        mode,
        super::Mode::Update(super::UpdateCommand {
            kind: super::UpdateCommandKind::Run {
                scope: cortex::update::UpdateScope::All,
                dry_run: false,
                profile: None,
                binary: None,
            },
            json: false,
        })
    ));
}

#[test]
fn mode_parse_accepts_update_server_dry_run_json_profile() {
    let mode = super::Mode::parse(vec![
        "update".into(),
        "server".into(),
        "--dry-run".into(),
        "--json".into(),
        "--profile".into(),
        "/tmp/deployments.toml".into(),
    ])
    .unwrap();

    assert!(matches!(
        mode,
        super::Mode::Update(super::UpdateCommand {
            kind: super::UpdateCommandKind::Run {
                scope: cortex::update::UpdateScope::Server,
                dry_run: true,
                profile: Some(ref profile),
                binary: None,
            },
            json: true,
        }) if profile == "/tmp/deployments.toml"
    ));
}

#[test]
fn mode_parse_accepts_update_clients_aliases() {
    for scope_name in ["clients", "agents"] {
        let mode = super::Mode::parse(vec!["update".into(), scope_name.into()]).unwrap();
        assert!(matches!(
            mode,
            super::Mode::Update(super::UpdateCommand {
                kind: super::UpdateCommandKind::Run {
                    scope: cortex::update::UpdateScope::Clients,
                    ..
                },
                ..
            })
        ));
    }
}

#[test]
fn mode_parse_accepts_update_config_server() {
    let mode = super::Mode::parse(vec![
        "update".into(),
        "config".into(),
        "server".into(),
        "--host".into(),
        "nashost".into(),
        "--home".into(),
        "/mnt/cache/appdata/cortex".into(),
        "--profile".into(),
        "/tmp/deployments.toml".into(),
        "--json".into(),
    ])
    .unwrap();

    assert!(matches!(
        mode,
        super::Mode::Update(super::UpdateCommand {
            kind: super::UpdateCommandKind::ConfigServer {
                ref host,
                ref home,
                profile: Some(ref profile),
            },
            json: true,
        }) if host == "nashost" && home == "/mnt/cache/appdata/cortex" && profile == "/tmp/deployments.toml"
    ));
}

#[test]
fn mode_parse_accepts_update_config_clients() {
    let mode = super::Mode::parse(vec![
        "update".into(),
        "config".into(),
        "clients".into(),
        "--hosts".into(),
        "devhost,backuphost".into(),
        "--target".into(),
        "https://cortex.example.internal".into(),
        "--docker".into(),
    ])
    .unwrap();

    assert!(matches!(
        mode,
        super::Mode::Update(super::UpdateCommand {
            kind: super::UpdateCommandKind::ConfigClients {
                ref hosts,
                target: Some(ref target),
                docker: Some(true),
                journald: None,
                profile: None,
            },
            json: false,
        }) if hosts == &vec!["devhost".to_string(), "backuphost".to_string()]
             && target == "https://cortex.example.internal"
    ));
}
```

- [ ] **Step 2: Run parser tests to verify they fail**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' mode_parse_accepts_update -- --nocapture
```

Expected: FAIL because `Mode::Update`, `UpdateCommand`, and parser support do not exist.

- [ ] **Step 3: Add update command structs and mode dispatch to `src/main.rs`**

Add `Mode::Update` to the main match:

```rust
Mode::Update(command) => run_update(command).await,
```

Add to `enum Mode`:

```rust
Update(UpdateCommand),
```

Add structs near `DeployCommand`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateCommand {
    kind: UpdateCommandKind,
    json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateCommandKind {
    Run {
        scope: cortex::update::UpdateScope,
        dry_run: bool,
        profile: Option<String>,
        binary: Option<String>,
    },
    ConfigServer {
        host: String,
        home: String,
        profile: Option<String>,
    },
    ConfigClients {
        hosts: Vec<String>,
        target: Option<String>,
        docker: Option<bool>,
        journald: Option<bool>,
        profile: Option<String>,
    },
}
```

Add parser arm before `setup`:

```rust
[command, rest @ ..] if command == "update" && global == cli::GlobalFlags::default() => {
    Ok(Self::Update(parse_update_command(rest)?))
}
```

- [ ] **Step 4: Add `parse_update_command()` and `run_update()` to `src/main.rs`**

```rust
async fn run_update(command: UpdateCommand) -> Result<()> {
    match command.kind {
        UpdateCommandKind::Run {
            scope,
            dry_run,
            profile,
            binary,
        } => {
            let report = cortex::update::run_update(
                scope,
                cortex::update::UpdateOptions {
                    dry_run,
                    profile_path: profile.map(std::path::PathBuf::from),
                    binary: binary.map(std::path::PathBuf::from),
                },
            )?;
            if command.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_update_report(&report);
            }
            if report.has_errors {
                anyhow::bail!("cortex update {} completed with failed phases", report.mode);
            }
        }
        UpdateCommandKind::ConfigServer { host, home, profile } => {
            let profile = cortex::update::configure_server_profile(
                profile.as_deref().map(std::path::Path::new),
                &host,
                &home,
            )?;
            if command.json {
                println!("{}", serde_json::to_string_pretty(&profile)?);
            } else {
                println!("cortex update config server");
                println!("host: {host}");
                println!("home: {home}");
            }
        }
        UpdateCommandKind::ConfigClients {
            hosts,
            target,
            docker,
            journald,
            profile,
        } => {
            let profile = cortex::update::configure_clients_profile(
                profile.as_deref().map(std::path::Path::new),
                hosts.clone(),
                target,
                docker,
                journald,
            )?;
            if command.json {
                println!("{}", serde_json::to_string_pretty(&profile)?);
            } else {
                println!("cortex update config clients");
                println!("hosts: {}", hosts.join(","));
            }
        }
    }
    Ok(())
}

fn print_update_report(report: &cortex::update::UpdateReport) {
    println!("cortex update {}", report.mode);
    println!("profile: {}", report.profile_path.display());
    if let Some(server) = &report.server {
        println!("server: {} {}", server.host, server.home);
        for phase in &server.phases {
            println!(
                "server\t{:?}\t{}\t{}ms\t{}",
                phase.status, phase.name, phase.elapsed_ms, phase.detail
            );
        }
    }
    for client in &report.clients {
        println!(
            "client\t{}\t{}\t{}ms\t{}",
            if client.ok { "ok" } else { "error" },
            client.host,
            client.elapsed_ms,
            client.detail
        );
    }
    for phase in &report.skipped {
        println!(
            "skip\t{:?}\t{}\t{}ms\t{}",
            phase.status, phase.name, phase.elapsed_ms, phase.detail
        );
    }
}

fn parse_update_command(args: &[String]) -> Result<UpdateCommand> {
    let mut json = false;
    let mut dry_run = false;
    let mut profile: Option<String> = None;
    let mut binary: Option<String> = None;
    let mut rest = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--dry-run" => dry_run = true,
            "--profile" => {
                i += 1;
                profile = Some(args.get(i).ok_or_else(|| anyhow::anyhow!("--profile requires a value"))?.clone());
            }
            "--binary" => {
                i += 1;
                binary = Some(args.get(i).ok_or_else(|| anyhow::anyhow!("--binary requires a value"))?.clone());
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => rest.push(other.to_string()),
        }
        i += 1;
    }

    if rest.first().map(String::as_str) == Some("config") {
        return parse_update_config_command(&rest[1..], json, profile);
    }

    let scope = match rest.as_slice() {
        [] => cortex::update::UpdateScope::All,
        [scope] if scope == "all" => cortex::update::UpdateScope::All,
        [scope] if scope == "server" => cortex::update::UpdateScope::Server,
        [scope] if scope == "clients" || scope == "agents" => cortex::update::UpdateScope::Clients,
        [other] => anyhow::bail!("unknown update scope: {other}"),
        _ => anyhow::bail!("update accepts at most one scope"),
    };
    Ok(UpdateCommand {
        kind: UpdateCommandKind::Run {
            scope,
            dry_run,
            profile,
            binary,
        },
        json,
    })
}
```

Add helper:

```rust
fn parse_update_config_command(
    args: &[String],
    json: bool,
    profile: Option<String>,
) -> Result<UpdateCommand> {
    let (target, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("update config requires server or clients"))?;
    match target.as_str() {
        "server" => {
            let mut host = None;
            let mut home = None;
            let mut i = 0usize;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--host" => {
                        i += 1;
                        host = Some(rest.get(i).ok_or_else(|| anyhow::anyhow!("--host requires a value"))?.clone());
                    }
                    "--home" => {
                        i += 1;
                        home = Some(rest.get(i).ok_or_else(|| anyhow::anyhow!("--home requires a value"))?.clone());
                    }
                    other => anyhow::bail!("unknown update config server argument: {other}"),
                }
                i += 1;
            }
            Ok(UpdateCommand {
                kind: UpdateCommandKind::ConfigServer {
                    host: host.ok_or_else(|| anyhow::anyhow!("update config server requires --host"))?,
                    home: home.ok_or_else(|| anyhow::anyhow!("update config server requires --home"))?,
                    profile,
                },
                json,
            })
        }
        "clients" | "agents" => {
            let mut hosts = Vec::new();
            let mut target = None;
            let mut docker = None;
            let mut journald = None;
            let mut i = 0usize;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--hosts" => {
                        i += 1;
                        hosts = rest
                            .get(i)
                            .ok_or_else(|| anyhow::anyhow!("--hosts requires a value"))?
                            .split(',')
                            .map(str::trim)
                            .filter(|host| !host.is_empty())
                            .map(str::to_string)
                            .collect();
                    }
                    "--target" => {
                        i += 1;
                        target = Some(rest.get(i).ok_or_else(|| anyhow::anyhow!("--target requires a value"))?.clone());
                    }
                    "--docker" => docker = Some(true),
                    "--journald" => journald = Some(true),
                    other => anyhow::bail!("unknown update config clients argument: {other}"),
                }
                i += 1;
            }
            Ok(UpdateCommand {
                kind: UpdateCommandKind::ConfigClients {
                    hosts,
                    target,
                    docker,
                    journald,
                    profile,
                },
                json,
            })
        }
        other => anyhow::bail!("unknown update config target: {other}"),
    }
}
```

- [ ] **Step 5: Run parser tests to verify they pass**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' mode_parse_accepts_update -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit Task 3**

```bash
git add src/main.rs src/main_tests.rs
git commit -m "feat: add cortex update cli"
```

---

### Task 4: Auto-Save Server Profile From Remote Deploy

**Files:**
- Modify: `src/main.rs`
- Modify: `src/main_tests.rs`

**Interfaces:**
- Consumes: Task 1 `configure_server_profile(None, host, home)`.
- Produces: successful non-dry-run `setup deploy remote --home PATH HOST` writes `<cortex-home>/deployments.toml`.

- [ ] **Step 1: Add a focused test for the parser shape already needed by profile save**

Append to `src/main_tests.rs`:

```rust
#[test]
fn parse_deploy_remote_home_has_enough_data_to_save_update_profile() {
    let command = super::parse_deploy_command(&[
        "remote".into(),
        "--home".into(),
        "/mnt/cache/appdata/cortex".into(),
        "nashost".into(),
    ])
    .unwrap();

    assert!(matches!(
        command.kind,
        super::DeployCommandKind::Remote {
            ref host,
            dry_run: false,
            home: Some(ref home),
        } if host == "nashost" && home == "/mnt/cache/appdata/cortex"
    ));
}
```

- [ ] **Step 2: Run the new deploy parser test**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' parse_deploy_remote_home_has_enough_data_to_save_update_profile -- --nocapture
```

Expected: PASS. This guards the data shape used by the next step.

- [ ] **Step 3: Save the profile after successful remote deploy**

In `run_deploy()`, inside the `DeployCommandKind::Remote` arm, after:

```rust
if report.has_errors {
    anyhow::bail!("cortex setup deploy remote {host} completed with failed phases");
}
```

insert:

```rust
if !dry_run {
    cortex::update::configure_server_profile(None, &host, &report.home)?;
}
```

This must happen only after the failed-phase bail, so failed remote deploys never overwrite the profile.

- [ ] **Step 4: Run deploy and update parser tests**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' 'parse_deploy_remote|mode_parse_accepts_update' -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit Task 4**

```bash
git add src/main.rs src/main_tests.rs
git commit -m "feat: remember remote server update profile"
```

---

### Task 5: Add Help And Documentation

**Files:**
- Modify: `src/cli/help.rs`
- Modify: `docs/CLI.md`
- Modify: `docs/mcp/DEPLOY.md`

**Interfaces:**
- Consumes: CLI shapes from Task 3.
- Produces: user-facing docs for the normal update workflow and the primitive deploy workflow.

- [ ] **Step 1: Add failing help test**

Append to `tests/cli_help.rs`:

```rust
#[test]
fn update_help_shows_server_clients_and_config_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_cortex"))
        .args(["update", "--help"])
        .output()
        .expect("run cortex update --help");

    assert!(output.status.success(), "update --help should exit 0");
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");
    assert!(stdout.contains("cortex update [all|server|clients|agents]"));
    assert!(stdout.contains("cortex update config server --host HOST --home PATH"));
    assert!(stdout.contains("cortex update config clients --hosts h1,h2"));
}
```

- [ ] **Step 2: Run the help test to verify it fails**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' --test cli_help update_help_shows_server_clients_and_config_commands -- --nocapture
```

Expected: FAIL because `update` help is not registered.

- [ ] **Step 3: Update `src/cli/help.rs`**

Add `update` to the top-level command catalog using the existing `CommandDoc` pattern:

```rust
CommandDoc {
    name: "update",
    summary: "Update the configured Cortex server and host-agent clients",
    usage: "cortex update [all|server|clients|agents] [--dry-run] [--json]",
},
```

Add this nested command entry to `NESTED_CATALOG`:

```rust
NestedCommandDoc {
    path: "update",
    summary: "Update the configured server and host-agent clients",
    usage: &[
        "cortex update [all|server|clients|agents] [--dry-run] [--json]",
        "cortex update server [--dry-run] [--json]",
        "cortex update clients [--json] [--binary PATH]",
        "cortex update agents [--json] [--binary PATH]",
        "cortex update config server --host HOST --home PATH [--json]",
        "cortex update config clients --hosts h1,h2 [--target URL] [--docker] [--journald] [--json]",
    ],
},
```

- [ ] **Step 4: Update `docs/CLI.md`**

Add this section before `### cortex setup deploy`:

````markdown
### `cortex update`

Update an already-configured Cortex deployment.

```bash
cortex update
cortex update server --dry-run
cortex update server
cortex update clients
cortex update agents
```

`cortex update` defaults to `all`: it updates the configured server first, then
updates configured host-agent clients. `clients` and `agents` are aliases; both
refer to the host-local Cortex agents that forward logs, heartbeats, sessions,
shell history, and command events into the server.

Configure the update profile once:

```bash
cortex update config server --host nashost --home /mnt/cache/appdata/cortex
cortex update config clients --hosts devhost,backuphost,edgehost --target https://cortex.example.internal --docker
```

The profile lives at `~/.cortex/deployments.toml` by default. A successful
`cortex setup deploy remote --home PATH HOST` also records the server profile,
so a one-off low-level deploy can seed future `cortex update server` runs.
````

- [ ] **Step 5: Update `docs/mcp/DEPLOY.md`**

Replace the paragraph that starts with “Nashost's canonical update path is” with:

````markdown
The normal update workflow is:

```bash
cortex update
cortex update server --dry-run
cortex update server
```

`cortex setup deploy remote --home /mnt/cache/appdata/cortex nashost` remains the
low-level primitive and a useful escape hatch. A successful low-level remote
deploy records the server profile so later updates do not repeat host/home
details.
````

- [ ] **Step 6: Run help and docs-adjacent tests**

Run:

```bash
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' --test cli_help -- --nocapture --test-threads=1
```

Expected: PASS.

- [ ] **Step 7: Commit Task 5**

```bash
git add src/cli/help.rs tests/cli_help.rs docs/CLI.md docs/mcp/DEPLOY.md
git commit -m "docs: document cortex update workflow"
```

---

### Task 6: End-To-End Verification And Live Profile Seeding

**Files:**
- Modify only if verification exposes a bug in files touched by earlier tasks.

**Interfaces:**
- Consumes: all previous tasks.
- Produces: pushed green branch with profile-backed `cortex update` behavior.

- [ ] **Step 1: Run full focused validation**

Run:

```bash
cargo fmt --check
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' update_ -- --nocapture
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' mode_parse_accepts_update -- --nocapture
RUSTC_WRAPPER='' cargo test --config 'build.rustc-wrapper=""' --test cli_help -- --nocapture --test-threads=1
RUSTC_WRAPPER='' cargo clippy --locked --config 'build.rustc-wrapper=""' -- -D warnings
RUSTC_WRAPPER='' cargo build --release --locked --config 'build.rustc-wrapper=""'
```

Expected: every command exits 0.

- [ ] **Step 2: Seed the live update profile through the implemented command**

Run from the implementation worktree with the release binary built in Step 1:

```bash
./.cache/cargo/release/cortex update config server --host nashost --home /mnt/cache/appdata/cortex --json
./.cache/cargo/release/cortex update server --dry-run --json
./.cache/cargo/release/cortex update server --json
```

Expected:
- `config server` writes a profile containing `server.host = "nashost"` and `server.home = "/mnt/cache/appdata/cortex"`.
- `update server --dry-run` reports `host: nashost`, `home: /mnt/cache/appdata/cortex`, and no errors.
- `update server` reports all phases ok and no errors.

- [ ] **Step 3: Verify live nashost server state**

Run:

```bash
ssh nashost "docker compose --env-file /mnt/cache/appdata/cortex/.env -f /mnt/cache/appdata/cortex/compose/docker-compose.yml config --images"
ssh nashost "docker ps --filter name=cortex --format '{{.Names}}\t{{.Image}}\t{{.Status}}'"
ssh nashost 'set -eu; token=$(grep "^CORTEX_API_TOKEN=" /mnt/cache/appdata/cortex/.env | cut -d= -f2-); curl -fsS -H "Authorization: Bearer ${token}" http://127.0.0.1:3100/api/version'
```

Expected:
- Compose resolves `ghcr.io/jmagar/cortex:3.9.1` or the current package version if it has changed.
- `cortex` container is healthy.
- `/api/version` returns the current package version and schema version.

- [ ] **Step 4: Run repo gate and push**

Run:

```bash
git status --short
bd close syslog-mcp-jlih1 --reason "Implemented cortex update operator workflow"
bd dolt push
git pull --rebase --autostash
git push
git status --short --branch
```

Expected:
- Bead is closed and pushed.
- Git branch is pushed.
- Worktree is clean except for intentionally unrelated dirty files if the coordinator worktree had any before the work-it checkout.

---

## Self-Review

- Spec coverage: The plan covers the requested `cortex update` command, server update without repeating host/home, configured host-agent clients, the low-level deploy escape hatch, profile persistence, JSON output, help, docs, tests, live verification, and Beads close-out.
- Placeholder scan: No unfinished markers or unspecified “add tests” steps remain. Every task has concrete file paths, commands, and code snippets.
- Type consistency: `UpdateScope`, `UpdateOptions`, `UpdateProfile`, `ServerUpdateProfile`, `ClientsUpdateProfile`, `UpdateReport`, and parser enum names are consistent across tasks.
