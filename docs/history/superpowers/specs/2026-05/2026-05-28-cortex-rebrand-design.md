# cortex v1.0.0 — Rebrand Design

**Date:** 2026-05-28
**Status:** Draft
**Scope:** Full rename of syslog-mcp → cortex

---

## Summary

Rename the `syslog-mcp` project to `cortex` at v1.0.0. The product has grown well beyond an MCP server — it now includes a full CLI, HTTP API with parity to MCP, fleet state, heartbeat agent, command log, AI watch, RAG endpoints, and OTLP ingestion. The name change is a declaration: cortex is a homelab intelligence platform that happens to expose MCP as one of its interfaces.

This is a hard break. No backwards-compatibility shims. Deployers update env vars and agent configs as part of the v1.0.0 upgrade.

---

## Decisions

| Question | Decision |
|---|---|
| Binary name | `cortex` (canonical) + `cx` (short alias) |
| MCP tool name | `cortex` — action strings unchanged |
| Env prefix | `CORTEX_*` (was `SYSLOG_MCP_*`) |
| Version | v1.0.0 (was v0.35.0) |
| Docker image | `ghcr.io/jmagar/cortex` canonical + Docker Hub mirror |
| GitHub repo | Rename `jmagar/syslog-mcp` → `jmagar/cortex` |
| Internal depth | Full: modules, types, config field+section+fns |
| DB file | Renamed `syslog.db` → `cortex.db` (data-file migration per host) |
| Plugin / data dir | Plugin `cortex` → `cortex` (`syslog-jmagar-lab/` → `cortex-jmagar-lab/`) |
| Migration | Hard break — no deprecation shims |
| Execution | Narrow script for `*syslog-mcp*` tokens + compiler-driven type renames + dedicated config task; single PR |

---

## Identity Changes

### External surfaces

| Surface | Before | After |
|---|---|---|
| Crate name | `syslog-mcp` | `cortex` |
| Binary | `cortex` | `cortex` + `cx` alias |
| MCP tool | `cortex` | `cortex` |
| Env prefix | `SYSLOG_MCP_*` | `CORTEX_*` |
| Docker image | `jmagar/syslog-mcp` | `ghcr.io/jmagar/cortex` |
| GitHub repo | `jmagar/syslog-mcp` | `jmagar/cortex` |
| Version | 0.35.0 | 1.0.0 |

### Internal renames

Only three `Syslog`-prefixed types actually exist in the codebase. (`SyslogEntry` /
`SyslogRecord` were assumed in an earlier draft but **do not exist** — the code already
uses protocol-neutral `LogEntry` / `LogRecord`, so there is nothing to rename there.)

| What | Before | After |
|---|---|---|
| Protocol module | `src/syslog/` | `src/receiver/` |
| Module file | `src/syslog.rs` | `src/receiver.rs` |
| Service type | `SyslogService` | `CortexService` |
| RMCP server type | `SyslogRmcpServer` | `CortexRmcpServer` |
| Config struct | `SyslogConfig` | `ReceiverConfig` |
| Config struct field | `Config.syslog` | `Config.receiver` |
| Config section | `[syslog]` | `[receiver]` |
| Config default fns | `default_syslog_host`, `default_syslog_port` | `default_receiver_host`, `default_receiver_port` |
| Config validator fn | `validate_syslog_config` | `validate_receiver_config` |
| Ingest helper fn | `from_syslog_config` | `from_receiver_config` |

The three type renames (`SyslogService`, `SyslogRmcpServer`, `SyslogConfig`) are driven
off `cargo check`, which surfaces every reference — not sed. The config field/section/fn
renames are coupled (see below) and verified by a config-parse test, since serde
mismatches compile clean and only fail at runtime.

### Config field/section coupling

`Config` has `pub syslog: SyslogConfig` with `#[serde(default)]` and **no serde alias**.
The TOML section `[syslog]` deserializes into that field. Renaming the section to
`[receiver]` therefore **requires** renaming the struct field `cortex` → `receiver` in
lockstep, or config loading silently falls back to defaults. `cargo check` will NOT catch
this — it is a runtime serde concern. The rename is verified by a parse test in
`src/config_tests.rs` that loads a `[receiver]` TOML and asserts the values land.

### Database file and plugin data dir

- SQLite file renamed `syslog.db` → `cortex.db`. This is a **data-file migration**: each
  deployed host must `mv` the existing DB (and checkpoint its WAL) during upgrade. Covered
  in the migration checklist.
- Plugin name `cortex` → `cortex`, which moves the plugin data dir
  (`…/plugins/data/syslog-jmagar-lab/` → `cortex-jmagar-lab/`). Existing installs must move
  their data dir or re-bootstrap.

### KEEP list — values that must NOT change

The rename script stays narrow: it only substitutes `syslog-mcp`, `syslog_mcp`, and
`SYSLOG_MCP_`. It never touches bare-word `cortex`. These must be preserved verbatim:

- **Source aliases** `"syslog-udp"` / `"syslog-tcp"` — stored in the DB and matched in code
  (`src/enrich/parser.rs`, `src/enrich/dispatch.rs`, `src/app/service.rs`). The
  `SourceKind::SyslogUdp` / `SyslogTcp` enum variants stay too (they map to those strings).
- **Facility filter values** (`kern`, `auth`, `daemon`, …) in MCP action descriptions.
- **RFC 3164/5424 protocol references** in comments and docs — "syslog" the protocol.

### What does NOT change

- All 42+ MCP action strings (`search`, `fleet_state`, `heartbeat`, etc.)
- SQLite schema and **on-disk format** (the file is renamed, not reformatted)
- HTTP API routes (`/api/v1/*`)
- RFC 3164/5424 wire protocol support
- Source-kind aliases (`syslog-udp`, `syslog-tcp`) — see KEEP list
- Docker Compose structure
- Justfile command names
- Beads issue tracker integration

---

## Rationale for key decisions

**`src/syslog/` → `src/receiver/`**: The module receives log streams over multiple protocols (RFC syslog, Docker Engine API, OTLP). "receiver" is protocol-neutral and accurate. "syslog" there referred to the RFC protocol, but with the product renamed the ambiguity becomes confusing.

**Log record types already protocol-neutral**: cortex ingests syslog, Docker logs, OTLP, and AI transcripts into a unified record. The code already names these `LogEntry` / `LogRecord` — no rename needed. (An earlier draft of this spec wrongly assumed `SyslogEntry` / `SyslogRecord` existed.)

**Hard break at v1.0.0**: The deployer (a single homelab) controls all instances. A deprecation window adds complexity with no practical benefit.

**Script-assisted single PR**: The rename is ~95% mechanical string substitution. A script makes every substitution auditable and reproducible. The remaining 5% (semantic renames, file moves) is done by hand after running the script.

**MCP action strings unchanged**: These are the stable API surface that agents depend on. The tool name changes (`cortex` → `cortex`) but all action names stay exactly as-is. Agents need one config update (tool name), not a full action remapping.

---

## Execution Plan

### Phase 1 — Write the rename script

`scripts/rename.sh` performs mechanical substitutions across all non-binary files:

```
SYSLOG_MCP_  → CORTEX_
syslog-mcp   → cortex   (Cargo.toml, docs, Docker, image names)
syslog_mcp   → cortex   (Rust lib name in `use` statements)
```

The script does NOT substitute bare-word `cortex`, type names, or config identifiers —
those are handled by the compiler-driven loop (type renames) and a dedicated config task
(field/section/fn renames with a parse test). See the KEEP list above for values that must
survive verbatim.

The script excludes:
- `target/` and `.git/`
- The CHANGELOG (updated manually)
- Anything on the KEEP list

### Phase 2 — File moves

```
src/syslog/   → src/receiver/
src/syslog.rs → src/receiver.rs
```

Update all `mod syslog` / `use crate::syslog` references to `mod receiver` / `use crate::receiver`.

### Phase 3 — Cargo.toml and binary

```toml
[package]
name = "cortex"

[[bin]]
name = "cortex"
path = "src/main.rs"

[[bin]]
name = "cx"
path = "src/main.rs"   # same entrypoint — Cargo builds two binaries from one source
```

### Phase 4 — MCP tool registration

In `src/mcp/rmcp_server.rs` and `src/mcp/schemas.rs`: rename the tool from `cortex` to `cortex`. Action dispatch table unchanged.

### Phase 5 — Docker and CI

- `docker-compose.yml` / `docker-compose.prod.yml`: image name, container name, service name
- `.github/workflows/`: image tags, release artifact names
- `install.sh`: binary name references
- `server.json`: MCP tool name

### Phase 6 — Docs

- `README.md`: product name, install instructions, binary name examples
- `CLAUDE.md` / `AGENTS.md` / `GEMINI.md`: all references
- `CHANGELOG.md`: add v1.0.0 entry documenting breaking changes
- `docs/`: sweep for `syslog-mcp` and `SYSLOG_MCP_` references

### Phase 7 — Plugin manifests

- `.claude-plugin/`: skill manifests, tool name references
- `plugins/`: any skill files referencing the old binary or tool name
- `mcpb/`: MCP builder config if present

### Phase 8 — Verify

```bash
cargo build --release          # must compile clean
just test                      # all tests pass
just lint                      # no clippy warnings
grep -r "syslog-mcp\|SYSLOG_MCP_\|SyslogService\|SyslogRmcpServer\|SyslogConfig" src/ --include="*.rs"
# expect: zero hits (bare-word "syslog" in RFC comments and KEEP-list aliases remain)
```

### Phase 9 — Release

```bash
# Cargo.toml already has version = "1.0.0"
git tag v1.0.0
git push origin v1.0.0
# GitHub Actions builds ghcr.io/jmagar/cortex:1.0.0
# Then rename repo on GitHub: jmagar/syslog-mcp → jmagar/cortex
```

---

## Deployment migration checklist

For each deployed instance after upgrading to v1.0.0 (service stopped first):

1. **Stop the service** so the DB has no open writers and the WAL can be checkpointed.
2. **Rename the DB file** in the data dir: checkpoint the WAL, then
   `mv syslog.db cortex.db` (and `mv syslog.db-wal cortex.db-wal`, `mv syslog.db-shm
   cortex.db-shm` if present). Simplest safe sequence:
   `sqlite3 syslog.db "PRAGMA wal_checkpoint(TRUNCATE);" && mv syslog.db cortex.db`
3. Update `.env`: rename all `SYSLOG_MCP_*` vars to `CORTEX_*`, and the compose-level
   `SYSLOG_PORT`/`SYSLOG_HOST`/`SYSLOG_UID`/`SYSLOG_GID`/`SYSLOG_HOST_PORT` to their
   `CORTEX_*` equivalents. Point `CORTEX_DB_PATH` at the renamed `cortex.db`.
4. Update `config.toml`: rename the `[syslog]` section to `[receiver]`.
5. Update agent configs: MCP tool name `cortex` → `cortex`.
6. Update `server.json` on MCP clients.
7. For plugin installs: move the data dir `…/plugins/data/syslog-jmagar-lab/` →
   `cortex-jmagar-lab/` (or re-run setup to re-bootstrap).
8. Update Docker Compose to pull `ghcr.io/jmagar/cortex:1.0.0`.
9. Start the service and verify (`cortex --http db status`).

---

## Out of scope

These are future cortex work, not part of the v1.0.0 rebrand:

- Web frontend / dashboard
- New MCP actions
- Architecture changes to the ingest pipeline
- Extracting cortex into a standalone open-source project
