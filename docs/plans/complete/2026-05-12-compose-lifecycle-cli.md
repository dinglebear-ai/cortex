# Compose Lifecycle CLI Plan

Date: 2026-05-12

## Goal

Add first-class `syslog compose ...` lifecycle commands for managing the cortex Docker Compose deployment, with the operational logic living in a shared library layer. CLI and MCP should be thin adapters over that shared layer.

The immediate problem to solve is deployment ownership ambiguity: a repo checkout can contain `docker-compose.yml`, while the live `cortex` container may be owned by a plugin data-dir Compose project such as `/home/jmagar/.claude/plugins/data/syslog-jmagar-lab`. The command must detect that state and make the safe target obvious.

## Non-Goals

- Do not reimplement Docker Compose in Rust.
- Do not make mutating lifecycle actions available over MCP by default.
- Do not expose raw rendered Compose config over MCP in the first pass.
- Do not require a running syslog database just to inspect or manage Compose state.
- Do not remove existing `just up`, `just down`, `just restart`, or `just logs` shortcuts.

## Architecture

Start with a shared module, tentatively:

```text
src/compose.rs
src/compose_tests.rs
```

Responsibilities:

- read-only Docker/Compose inspection
- subprocess wrapper for `docker compose ...`
- shared orchestration API used by CLI and MCP
- request/response structs with `Serialize` support

Split into `src/compose/{models,docker,cli_runner,service}.rs` only after the implementation grows enough to justify it. The first pass should optimize for a small, auditable control surface rather than a broad module tree.

Keep command parsing and printing in `src/cli.rs`. Keep MCP argument decoding and schema exposure in `src/mcp/tools.rs` and `src/mcp/schemas.rs`. Those layers should call `compose::service` and format or serialize the returned models.

## Shared API

Add a service object that does not depend on `SyslogService` or the SQLite pool:

```rust
pub struct ComposeService {
    docker: DockerInspector,
    runner: ComposeRunner,
    defaults: ComposeDefaults,
}
```

Core request/response shapes:

```rust
pub struct ComposeTarget {
    pub project_dir: Option<PathBuf>,
    pub compose_file: Option<PathBuf>,
    pub project_name: Option<String>,
    pub service: Option<String>,
    pub container_name: Option<String>,
}

pub enum TargetSource {
    Explicit,
    LiveContainerLabels,
    CurrentWorkingDirectory,
}

pub enum TargetConfidence {
    Confirmed,
    Ambiguous,
    Unsafe,
}

pub struct ResolvedComposeTarget {
    pub target: ComposeTarget,
    pub source: TargetSource,
    pub confidence: TargetConfidence,
    pub warnings: Vec<ComposeDiagnostic>,
    pub compose_files: Vec<PathBuf>,
    pub compose_working_dir: Option<PathBuf>,
    pub compose_project: Option<String>,
}

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

pub struct ComposeMcpStatus {
    pub container_name: String,
    pub ownership: ComposeOwnershipState,
    pub runtime_state: ComposeRuntimeState,
    pub health: Option<String>,
    pub published_ports: Vec<PublicPortSummary>,
    pub diagnostics: Vec<ComposeMcpDiagnostic>,
}

pub struct ComposeCommandResult {
    pub command: Vec<String>,
    pub exit_status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub timeout_cleanup: Option<TimeoutCleanupStatus>,
    pub status_after: Option<ComposeStatus>,
}
```

`ComposeStatus` is the local/CLI model and may include host paths and mount metadata. MCP must not serialize it directly; MCP uses `ComposeMcpStatus`, a coarse redacted projection. Prefer command-specific request structs over a single broad operation enum when behavior differs, especially for `logs` and `upgrade`. Use `ServiceResult`-style errors or a dedicated `ComposeError` that maps cleanly into both CLI errors and MCP JSON errors.

## Discovery Rules

The shared service should resolve targets in this order for read-only CLI operations:

1. Explicit `--compose-file`, `--project-dir`, or `--project-name`.
2. Existing container named `cortex`, inspected through the `DockerInspect` abstraction.
3. Compose labels on that container:
   - `com.docker.compose.project`
   - `com.docker.compose.project.working_dir`
   - `com.docker.compose.project.config_files`
   - `com.docker.compose.service`
4. Current working directory if it contains `docker-compose.yml`.
5. Error with a concrete explanation and suggested flags.

Target resolution must return the resolved target, the source of that target, confidence, and diagnostics. If a live container exists but its Compose labels point outside the current repo, `doctor` should report that as an ownership mismatch, not as a failure.

For mutating commands, current-working-directory fallback is not enough by itself. If the target source is only `CurrentWorkingDirectory`, the command must require an explicit confirmation flag such as `--allow-cwd-target` unless the user also supplied `--compose-file`, `--project-dir`, or `--project-name`.

For mutating commands, `--project-name` alone is not a safe explicit target because it still depends on cwd for compose-file and `.env` resolution. Mutations require either:

- matching live Compose labels, or
- explicit `--compose-file`, or
- explicit `--project-dir`.

If live ownership and the requested/cwd ownership disagree, mutating commands must refuse by default and print both targets. Intentional overrides must be explicit: `--allow-cwd-target` permits cwd fallback; `--allow-foreign-project` permits owner/image mismatch after preflight.

The service must canonicalize compose paths before comparing them, validate that label-referenced compose files still exist, and reject surprising symlink/path mismatches for mutating commands unless explicitly overridden.

When exact container-name lookup fails, `status` and `doctor` should also look for candidate containers by Compose labels such as `com.docker.compose.service=cortex` and `io.modelcontextprotocol.server.name=tv.nashost/cortex`. If multiple candidates exist, report ambiguity and do not pick one for mutation without explicit target flags.

## Mutation Safety Rules

Before `up`, `down`, `restart`, or `pull`, the shared service must run a mutation preflight. The same rules apply to deferred `upgrade` when it is added:

- Resolve and print the target project, compose file(s), project directory, service, and container name.
- Refuse ambiguous targets unless explicit target flags are present.
- Refuse cwd-vs-live-owner mismatches unless an explicit override is present.
- Validate that the resolved Compose config defines the expected service.
- Validate that the resolved service image or labels identify cortex, unless `--allow-foreign-project` is present.
- Refuse stale label paths or missing compose files.
- First pass checks only `cortex.service` plus live listeners on `1514` and `3100`.
- Refuse `up` and `restart` when `cortex.service` is active or when a non-target process owns `1514` or `3100`, unless explicitly overridden.
- Warn, but do not refuse, `pull` for systemd/listener conflicts because it does not change the running process.
- Allow `down` only against a confirmed Compose target; do not let systemd/listener detection redirect `down` toward anything else.
- For destructive commands such as `down`, require `--yes` in non-interactive mode after printing the target summary.
- `--dry-run` must run all resolution and preflight checks but never invoke the mutating runner.

`doctor` should report systemd and listener ownership even when no Compose container exists. It should distinguish:

- healthy Compose-owned deployment
- Compose owner differs from cwd
- systemd-owned deployment
- port listener exists but owner is unknown
- no live owner found
- Docker unavailable or permission denied

## Bollard vs Compose CLI

Use a `DockerInspect` abstraction for read-only inspection. First-pass implementation may use Docker CLI subprocesses (`docker inspect`, `docker ps`) instead of Bollard if that keeps Docker context behavior identical to `docker compose`. Bollard can remain a supported implementation because it is already a dependency, but the shared service must not depend directly on Bollard-specific types.

Read-only inspection needs:

- Find container by exact name `/cortex`.
- Read labels, image ID, health, restart policy, mounts, ports, network mode.
- Optionally inspect image labels and creation metadata after core status/doctor behavior is stable.

Use `docker compose` subprocesses for lifecycle mutations and rendered config:

- `up`
- `down`
- `restart`
- `pull`
- `logs`
- `upgrade`

Reason: Compose owns project resolution, env interpolation, config hashes, recreate behavior, volumes, networks, profiles, and orphan handling. Reimplementing those through the Docker Engine API would be brittle.

Keep Docker access consistent enough to diagnose context mismatches. If one inspection path cannot connect but the Compose CLI can, `status` and `doctor` should surface that exact mismatch instead of silently falling back to cwd mutation.

## Subprocess and Output Rules

All subprocess-backed operations must be bounded unless explicitly streaming:

- Non-streaming commands use per-operation timeouts.
- Captured stdout and stderr have byte caps and truncation flags.
- stdout and stderr must be drained concurrently into bounded buffers.
- After a stream reaches its cap, the runner continues draining and discarding data so the child cannot block on a full pipe.
- Captured output is redacted before human, JSON, or MCP output.
- Redaction must cover keys and values matching `token`, `secret`, `key`, `password`, `client_secret`, `authorization`, and similar credential terms.
- Timeout errors include command, elapsed time, exit status when available, and partial redacted stderr/stdout.
- On Unix, commands should run in their own process group/session when practical.
- On timeout, the runner sends a graceful termination signal, escalates to kill if needed, waits/reaps the process, and reports whether cleanup completed.
- The runner must avoid interactive prompts where possible and must not imply upgrade success when pull/build/up failed or timed out.

Lifecycle command mapping must be explicit and non-interactive:

- `syslog compose up` -> `docker compose up -d SERVICE`
- `syslog compose restart` -> `docker compose restart SERVICE`
- `syslog compose pull` -> `docker compose pull SERVICE`
- `syslog compose down` -> `docker compose down` after confirmed target and required `--yes` in non-interactive mode

`logs --follow` is a special streaming CLI path. It must not be represented as `ComposeCommandResult { stdout: String }`, must not support `--json`, and must stream directly to the terminal with normal user interrupt handling. First-pass implementation may defer `--follow` entirely and support only bounded `logs --tail N`.

## Compose Invocation Semantics

The runner must preserve Docker Compose project semantics exactly:

- Execute with the resolved Compose working directory as `current_dir`, or pass an equivalent `--project-directory`.
- Include every resolved compose file in Compose label order using repeated `-f FILE` arguments.
- Preserve Compose's `.env`, relative `build.context`, and relative bind-mount behavior for the resolved project, not the caller's cwd.
- Include `--project-name NAME` only when it came from live labels or explicit flags.
- Tests must prove a plugin data-dir target cannot accidentally resolve relative paths from the repo checkout.

## CLI Surface

Add `compose` as a top-level CLI namespace:

```bash
syslog compose status [--json] [--container NAME]
syslog compose doctor [--json]
syslog compose up [--dry-run] [--json]
syslog compose down [--dry-run] [--json]
syslog compose restart [--dry-run] [--json]
syslog compose pull [--dry-run] [--json]
syslog compose logs [--tail N]
```

Common target flags:

```bash
--compose-file PATH
--project-dir PATH
--project-name NAME
--service NAME      # default: cortex
--container NAME    # default: cortex
--allow-cwd-target
--allow-foreign-project
--yes               # required for destructive non-interactive mutations
```

`syslog compose config` is deferred from the first pass. The implementation may call `docker compose config` internally for preflight validation, but must not expose rendered config as a user-facing CLI or MCP command yet.

`upgrade` is deferred from the first pass. Document the safe two-step flow instead:

```bash
syslog compose pull
syslog compose up
```

When `upgrade` is added later:

1. Resolve the owning Compose project.
2. Run `docker compose pull SERVICE`.
3. Run `docker compose up -d --force-recreate SERVICE`.
4. Prefer Docker health status for the resolved target container.
5. If HTTP health is needed, derive the URL from inspected published ports, or use explicit `--health-url`.
6. Reinspect the target and print image, health, data mount, and Compose owner.
7. If health cannot be tied to the resolved target, report "started but health unverified" instead of success.

`upgrade --build` must require explicit `--build`, preflight the service build context, and refuse plugin data-dir projects whose relative build context cannot be verified.

## MCP Surface

Add read-only actions to the existing single `syslog` MCP tool:

```text
compose_status
compose_doctor
```

Do not add mutating MCP actions in the first pass.

Do not expose raw `docker compose config` over MCP in the first pass. If config inspection is later added, it must be redacted, bounded, and represented structurally rather than as raw rendered YAML.

First-pass MCP compose actions must reject caller-supplied target overrides. MCP must not accept arbitrary `container_name`, `project_dir`, `compose_file`, or `project_name` arguments. It may only inspect the canonical cortex deployment target selected by server-side defaults.

Compose MCP actions are operational diagnostics, not normal log reads. A dedicated operational read scope is deferred. First pass may use the existing read-scope policy only if the MCP adapter returns a conservative `ComposeMcpStatus` projection.

MCP responses must be built through a dedicated redaction/projection mapper:

- Do not serialize `ComposeStatus` directly.
- Omit host paths, bind-mount source paths, raw labels, raw command output, image IDs, compose file paths, and arbitrary diagnostics by default.
- Return coarse states such as `compose_owned`, `owner_mismatch`, `systemd_conflict`, `healthy`, `degraded`, and `docker_unavailable`.
- Include published service ports only as minimal service-facing summaries.

If mutating MCP actions are later required, gate them behind all of:

- `CORTEX_ADMIN_ACTIONS=true`
- mounted OAuth auth, not `LoopbackDev`
- no bearer-only/static-token admin mode for host lifecycle control
- caller has admin scope
- loopback/private-network binding policy
- per-action audit logging
- explicit refusal when the server process is running inside the target Compose project unless a one-shot local confirmation mechanism is present

This keeps remote MCP useful for diagnosis while preserving the CLI as the lifecycle control surface.

## CLI/Main Wiring

`src/main.rs` currently loads `RuntimeCore::load_query_only()` for every `Mode::Cli`. Compose commands should not require DB config or a readable SQLite database.

Implementation option:

```rust
enum CliCommand {
    Query(QueryCommand),
    Compose(ComposeCommand),
}
```

Then:

- Query commands continue through `RuntimeCore::load_query_only()`.
- Compose commands construct `ComposeService::default()` directly.

This avoids breaking lifecycle management when the database is corrupt, missing, or locked.

## Output

Human output should prioritize the operational facts:

```text
Container: cortex
Status: Up 2 minutes (healthy)
Image: ghcr.io/jmagar/cortex:latest
Compose project: syslog-jmagar-lab
Compose file: /home/jmagar/.claude/plugins/data/syslog-jmagar-lab/docker-compose.yml
Data: /home/jmagar/.claude/plugins/data/syslog-jmagar-lab -> /data
Ports: 1514/tcp, 1514/udp, 3100/tcp
Docker health: healthy
```

`--json` should return the shared model directly.

Human and JSON output must mark degraded/unsafe states clearly:

- Docker unavailable or permission denied
- Compose CLI unavailable or unsupported
- live owner differs from cwd
- systemd/listener conflict
- stale compose label paths
- output redacted or truncated
- health unverified

## Tests

Unit tests:

- Parse `syslog compose ...` commands in `src/cli_tests.rs`.
- Parse top-level mode in `src/main_tests.rs`.
- Resolve owner from fake Docker labels in `src/compose_tests.rs`.
- Verify mismatch diagnostics when current repo differs from label working dir.
- Verify mutating commands refuse cwd fallback without explicit confirmation.
- Verify mutating commands refuse live-owner/cwd mismatch without explicit override.
- Verify stale label paths and symlink surprises are refused for mutation.
- Verify systemd/listener conflict diagnostics and mutation refusal.
- Verify command vector construction for `docker compose -f PATH ...`, all label-reported compose files in order, and resolved project directory/current-dir semantics.
- Verify `--dry-run` does not invoke the runner.
- Verify subprocess timeout, output truncation, and redaction behavior.
- Verify timeout cleanup terminates and reaps the child process tree and reports cleanup status.
- Verify stdout/stderr are drained concurrently and discarded after caps without deadlock.
- Verify bounded `logs --tail` captures only capped output.
- Verify `logs --follow`, if implemented, is CLI-only streaming and rejects `--json`.
- Verify compose commands do not require the query runtime or a readable SQLite DB.
- Verify `up` maps to detached `docker compose up -d SERVICE`.
- Verify `--project-name` alone is rejected for mutating commands unless matching live labels provide a safe target.
- Verify first-pass user-facing `syslog compose config` is absent.

MCP tests:

- `compose_status` and `compose_doctor` actions are present in schema enum.
- `compose_config` is absent in the first pass.
- MCP tool handler calls the shared service and serializes response.
- Mutating compose action names are rejected or absent.
- MCP compose actions reject target override arguments.
- MCP compose output uses `ComposeMcpStatus`, not raw `ComposeStatus`.
- MCP compose output omits host paths, mount source paths, image IDs, raw labels, and raw command output.
- MCP compose actions use the existing read-scope behavior only with the conservative response projection.

Use traits for Docker inspection and command running so tests do not require a live Docker daemon:

Keep the test seam as simple as the implementation allows. If first-pass inspection and command execution are subprocess-backed, synchronous traits are enough:

```rust
trait DockerInspect {
    fn inspect_container(&self, name: &str) -> Result<Option<ContainerInfo>>;
}

trait CommandRunner {
    fn run(&self, command: ComposeInvocation) -> Result<CommandOutput>;
}
```

If a Bollard-backed implementation requires async later, use boxed futures or async traits only at that point. Do not make unit tests depend on host Docker state.

## Documentation

Update:

- `README.md` command table.
- `docs/mcp/DEPLOY.md` for lifecycle management.
- `docs/runbooks/deploy.md` to prefer `syslog compose doctor`, `syslog compose pull`, and `syslog compose up`.
- `AGENTS.md` or generated project docs if they list operational commands.

Document the recommended assistant workflow:

```bash
syslog compose doctor
syslog compose status
syslog compose pull
syslog compose up
```

## Verification

Minimum verification for implementation:

```bash
cargo fmt
cargo test
cargo clippy -- -D warnings
syslog compose status --json
syslog compose doctor
syslog compose logs --tail 20
```

Live verification, when safe:

```bash
syslog compose pull --dry-run
syslog compose up --dry-run
curl -fsS http://localhost:3100/health
```

Do not run non-dry-run lifecycle mutations as part of automated tests.

## Rollout Sequence

1. Add shared compose models, inspector trait, runner trait, and service in a small initial `src/compose.rs`.
2. Implement an inspector behind the trait, preferring Docker CLI subprocesses if needed to match Compose context semantics exactly.
3. Implement subprocess-backed Compose runner.
4. Add target resolution, mutation safety preflight, systemd/listener diagnostics, and redaction utilities.
5. Add explicit Compose invocation semantics: resolved project directory, all compose files in order, detached `up -d`, bounded `pull/restart/down`, process-group timeout cleanup, and concurrent pipe draining.
6. Add CLI parsing and human/JSON output for status, doctor, bounded logs, pull, up, down, and restart.
7. Split CLI execution so Compose commands do not load the query runtime.
8. Add read-only MCP `compose_status` and `compose_doctor` actions with canonical target selection and `ComposeMcpStatus` projection.
9. Update docs and run verification.
10. Add `config`, `upgrade`, `logs --follow`, and dedicated ops scope later only after core status/doctor/pull/up behavior is stable.

## Open Questions

- Should the default container name be configurable through an env var, or remain hard-coded as `cortex` with only a CLI flag override?
- Should `compose logs --follow` be deferred entirely, or implemented as CLI-only streaming in the first pass?
- Should a dedicated MCP operational read scope be introduced after the first pass, and what exact name should it use?
