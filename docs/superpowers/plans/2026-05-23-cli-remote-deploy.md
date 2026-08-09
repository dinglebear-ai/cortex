# CLI Remote Deploy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `syslog deploy remote <host>` as a CLI-only SSH deploy flow for the same Docker Compose server bundle used by local setup.

**Architecture:** Keep deploy mutation out of REST and MCP entirely. Add a focused `syslog_mcp::deploy` library module for remote SSH orchestration, wire `src/main.rs` to parse and print the command, and reuse existing setup assets (`docker-compose.prod.yml`, `config/Dockerfile`, token/env defaults) rather than adding a second deployment model.

**Tech Stack:** Rust 2021, `std::process::Command`, OpenSSH CLI (`ssh`), Docker Compose on the remote host, existing `SetupReport`/`SetupPhase` report shapes, sidecar unit tests.

---

## Scope Boundary

Implement:

- `syslog deploy remote <host> [--dry-run] [--json]`
- Remote preflight via `ssh -o BatchMode=yes -o ConnectTimeout=10 <host> ...`
- Remote repair/deploy via SSH, writing `~/.syslog-mcp/.env`, `~/.syslog-mcp/compose/docker-compose.yml`, and `~/.syslog-mcp/compose/config/Dockerfile`
- Remote Docker network check/create, `docker compose pull --ignore-buildable`, and `docker compose up -d`
- Redacted, audit-friendly phase output
- Docs and tests

Do not implement:

- REST deploy endpoints
- MCP deploy actions
- Background deploy jobs
- Remote host auto-mutation without an explicit `<host>` argument
- systemd deployment

## File Structure

- Modify `src/main.rs`: parse `deploy remote`, add remote variant to `DeployCommandKind`, dispatch to the new module, update usage text.
- Create `src/deploy.rs`: CLI-only remote deployment orchestration, command runner abstraction, redaction helpers, tests.
- Modify `src/lib.rs`: export `pub mod deploy`.
- Modify `src/setup.rs`: re-export a small asset/env helper surface needed by `deploy.rs` without exposing REST/MCP surfaces.
- Modify `src/setup/firstrun.rs`: make the existing setup asset/env helpers visible to `deploy.rs` where needed.
- Modify `docs/CLI.md` and `docs/mcp/DEPLOY.md`: document the CLI-only remote flow and explicitly state REST/MCP deploy is out of scope.
- Modify `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`, `.claude-plugin/plugin.json`, and `server.json`: patch bump per repo rule.

## Task 1: Parse Remote Deploy

**Files:**
- Modify: `src/main.rs`
- Test: `src/main_tests.rs`

- [ ] **Step 1: Write parser tests**

Add tests near the existing deploy parser tests:

```rust
#[test]
fn mode_parse_accepts_remote_deploy_namespace() {
    assert!(matches!(
        Mode::parse(vec![
            "deploy".into(),
            "remote".into(),
            "nashost".into(),
            "--dry-run".into(),
            "--json".into()
        ])
        .unwrap(),
        Mode::Deploy(_)
    ));
}

#[test]
fn mode_parse_rejects_remote_deploy_without_host() {
    let err = Mode::parse(vec!["deploy".into(), "remote".into()]).unwrap_err();
    assert!(err.to_string().contains("deploy remote requires a host"));
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test mode_parse_accepts_remote_deploy_namespace mode_parse_rejects_remote_deploy_without_host
```

Expected: the first test fails because `remote` is still unknown.

- [ ] **Step 3: Add the parser model**

Update `DeployCommandKind`:

```rust
enum DeployCommandKind {
    Preflight,
    Local { dry_run: bool },
    Remote { host: String, dry_run: bool },
}
```

Update `parse_deploy_command` so:

- `preflight` accepts only `--json`
- `local` accepts `--dry-run` and `--json`
- `remote` requires exactly one non-flag host and accepts `--dry-run` and `--json`
- any second host produces `deploy remote accepts exactly one host`

Use explicit parsing instead of treating every positional as unknown.

- [ ] **Step 4: Run parser tests**

Run:

```bash
cargo test deploy
```

Expected: deploy parser tests pass.

## Task 2: Add Remote Deploy Library

**Files:**
- Create: `src/deploy.rs`
- Modify: `src/lib.rs`
- Modify: `src/setup.rs`
- Modify: `src/setup/firstrun.rs`

- [ ] **Step 1: Write unit tests for command sequence and dry-run**

Create `src/deploy.rs` with tests using a fake runner. The tests should assert:

```rust
#[test]
fn remote_dry_run_only_checks_ssh_and_docker() {
    let mut runner = FakeRemoteRunner::ok();
    let report = run_remote_deploy_with_runner("host-a", true, &mut runner).unwrap();
    assert_eq!(report.host, "host-a");
    assert!(runner.commands.iter().any(|cmd| cmd.contains("docker --version")));
    assert!(!runner.commands.iter().any(|cmd| cmd.contains("docker compose up -d")));
    assert!(!runner.commands.iter().any(|cmd| cmd.contains("cat > ~/.syslog-mcp/.env")));
}

#[test]
fn remote_repair_writes_assets_before_compose_up() {
    let mut runner = FakeRemoteRunner::ok();
    let report = run_remote_deploy_with_runner("host-a", false, &mut runner).unwrap();
    assert!(!report.has_errors);
    let joined = runner.commands.join("\n");
    assert!(joined.contains("mkdir -p ~/.syslog-mcp/compose/config ~/.syslog-mcp/data"));
    assert!(joined.contains("cat > ~/.syslog-mcp/.env.tmp"));
    assert!(joined.contains("cat > ~/.syslog-mcp/compose/docker-compose.yml.tmp"));
    assert!(joined.contains("docker compose"));
    assert!(joined.contains("up -d"));
}
```

- [ ] **Step 2: Add public types**

Implement:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct RemoteDeployReport {
    pub mode: &'static str,
    pub host: String,
    pub home: String,
    pub env_path: String,
    pub compose_dir: String,
    pub data_dir: String,
    pub health_url: String,
    pub mcp_url: String,
    pub phases: Vec<crate::setup::SetupPhase>,
    pub has_errors: bool,
}

pub fn run_remote_deploy(host: &str, dry_run: bool) -> io::Result<RemoteDeployReport>
```

The report should mirror `SetupReport` enough for current CLI output patterns.

- [ ] **Step 3: Add runner abstraction**

Implement:

```rust
trait RemoteRunner {
    fn run(&mut self, host: &str, script: &str, stdin: Option<&str>) -> io::Result<RemoteOutput>;
}

struct SshRemoteRunner;

#[derive(Debug, Clone)]
struct RemoteOutput {
    status_success: bool,
    stdout: String,
    stderr: String,
}
```

`SshRemoteRunner` should execute:

```bash
ssh -o BatchMode=yes -o ConnectTimeout=10 <host> sh -s
```

and pass the script through stdin. Do not put secrets or full env content on the command line.

- [ ] **Step 4: Add deploy phases**

Implement phases:

- `ssh`: `true`
- `remote-filesystem`: check-only uses `test -d ~/.syslog-mcp || true`; repair creates dirs and chmods home/data
- `remote-env`: skipped in dry-run; repair writes `.env.tmp`, chmods it, then renames it
- `remote-compose-assets`: skipped in dry-run; repair writes compose and Dockerfile temp files then renames them
- `remote-docker`: `docker --version && docker compose version`
- `remote-docker-network`: skipped in dry-run; repair creates `${DOCKER_NETWORK:-syslog-mcp}` if absent
- `remote-compose-pull`: skipped in dry-run
- `remote-compose-up`: skipped in dry-run
- `remote-health`: skipped in dry-run; repair checks `curl -fsS http://127.0.0.1:${SYSLOG_MCP_PORT:-3100}/health`

Use `SetupStatus::Skipped` for dry-run mutation phases.

- [ ] **Step 5: Reuse setup assets**

Expose from setup:

```rust
pub fn installed_compose_asset() -> String
pub fn default_env_for_data_dir(data_dir: &Path) -> io::Result<BTreeMap<String, String>>
pub fn render_env(values: &BTreeMap<String, String>) -> String
pub fn dockerfile_asset() -> &'static str
```

The remote flow should generate env content locally using the same defaults and token generation as local setup, then send it over SSH stdin.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test deploy
```

Expected: new deploy module tests and parser tests pass.

## Task 3: Wire CLI Output

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Dispatch remote deploy**

Update `run_deploy`:

```rust
DeployCommandKind::Remote { host, dry_run } => {
    let report = syslog_mcp::deploy::run_remote_deploy(&host, dry_run)?;
    print_remote_deploy_report(&report, command.json)?;
    if report.has_errors {
        anyhow::bail!("syslog deploy remote {host} completed with failed phases");
    }
    return Ok(());
}
```

- [ ] **Step 2: Add human output helper**

Print:

```text
syslog deploy remote <host>
mode: remote dry-run|remote
host: <host>
home: ~/.syslog-mcp
env: ~/.syslog-mcp/.env
compose: ~/.syslog-mcp/compose
data: ~/.syslog-mcp/data
health: http://127.0.0.1:3100/health
mcp: http://127.0.0.1:3100/mcp
```

Then print phases with the existing `status name elapsed detail` shape.

- [ ] **Step 3: Update usage**

Add:

```text
syslog deploy remote HOST [--dry-run] [--json]
```

in both usage blocks.

- [ ] **Step 4: Run focused parser and output-safe tests**

Run:

```bash
cargo test deploy
```

Expected: all deploy tests pass.

## Task 4: Document CLI-Only Remote Deploy

**Files:**
- Modify: `docs/CLI.md`
- Modify: `docs/mcp/DEPLOY.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Update CLI docs**

In `docs/CLI.md`, update `syslog deploy` examples:

```bash
syslog deploy remote nashost --dry-run
syslog deploy remote nashost --json
```

State:

```markdown
`deploy remote` uses SSH and Docker Compose on the target host. It is CLI-only and requires an explicit host argument. It does not add REST or MCP deploy mutation surfaces.
```

- [ ] **Step 2: Update deployment guide**

In `docs/mcp/DEPLOY.md`, add a remote section:

```markdown
### Remote CLI Deploy

`syslog deploy remote <host>` copies the managed Compose bundle to
`~/.syslog-mcp` on the SSH target and runs Docker Compose there. Use
`--dry-run` first to verify SSH and Docker prerequisites.

Deploy mutations remain CLI-only. MCP exposes only read-only diagnostics.
```

- [ ] **Step 3: Add changelog entry**

Patch bump to `0.28.2` and add:

```markdown
## [0.28.2] - 2026-05-23

### Added

- **Remote deploy CLI**: Add `syslog deploy remote <host>` for SSH-based
  Compose deployment without adding REST or MCP mutation surfaces.
```

## Task 5: Version Bump and Full Verification

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `.claude-plugin/plugin.json`
- Modify: `server.json`

- [ ] **Step 1: Patch bump**

Change every current version-bearing file from `0.28.1` to `0.28.2`; update `server.json` image tag to `ghcr.io/jmagar/syslog-mcp:v0.28.2`.

- [ ] **Step 2: Refresh lockfile**

Run:

```bash
cargo check
```

Expected: succeeds and updates `Cargo.lock` package version.

- [ ] **Step 3: Run focused gates**

Run:

```bash
cargo fmt --check
bash scripts/check-version-sync.sh --require-changelog
bash scripts/validate-marketplace.sh
cargo test deploy
```

Expected: all pass.

- [ ] **Step 4: Run full gates**

Run:

```bash
cargo clippy -- -D warnings
cargo test
```

Expected: all pass. If `just check` still fails on the pre-existing module-size guard, record it separately and do not hide it.

## Self-Review Checklist

- [ ] No REST route added.
- [ ] No MCP action/schema added.
- [ ] No systemd deploy option added.
- [ ] Remote deploy requires an explicit host.
- [ ] Dry-run does not write files or start Compose.
- [ ] Secrets/env content are passed over SSH stdin, not command-line args or phase details.
- [ ] Docs state deploy mutation remains CLI-only.

## Implementation Status

Implemented on branch `feat/cli-remote-deploy`.

- [x] Added `syslog deploy remote HOST [--dry-run] [--json]` parser coverage.
- [x] Added CLI-only SSH remote deploy orchestration in `src/deploy.rs`.
- [x] Kept REST and MCP deploy mutation surfaces out of scope.
- [x] Added docs for remote deploy behavior and `.env` overwrite semantics.
- [x] Bumped version-bearing files to `0.28.2`.
- [x] Verified focused deploy tests, full test suite, clippy, formatting, version sync, and plugin validation.
