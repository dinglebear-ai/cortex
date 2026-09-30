# Hive Rebrand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebrand `syslog-mcp` to Hive while preserving the syslog protocol domain, protecting existing operator data, and providing compatibility aliases where users or clients already depend on `syslog` names.

**Architecture:** The project becomes a Hive MCP service with a primary `hive-mcp` Cargo package, `hive` binary, `hive` MCP tool, `hive://` schema resource, and `hive:*` scopes. Legacy `syslog` tool/resource/scope/env/binary compatibility remains during this release where it prevents breakage. Docker defaults move to Hive naming only with explicit migration handling so existing data directories, volumes, and networks are not silently abandoned.

**Tech Stack:** Rust, Cargo, RMCP Streamable HTTP, Axum, SQLite/FTS5, Docker Compose, Claude/Codex plugin manifests, Beads, GitHub CLI, Lavra, PR Review Toolkit.

---

## Execution Contract

- [ ] Create and enter a fresh worktree before implementation:

  ```bash
  cd /home/jmagar/workspace/syslog-mcp
  git fetch origin
  mkdir -p .worktrees
  git worktree add .worktrees/hive-rebrand -b hive-rebrand origin/main
  cd .worktrees/hive-rebrand
  ```

- [ ] Copy required local-only config files from the source checkout into the worktree without adding them to git:

  ```bash
  for f in .env config.toml CLAUDE.md.local AGENTS.local.md; do
    [ -f "../../$f" ] && cp "../../$f" "$f"
  done
  if [ -f "../../.cargo/config.toml" ]; then
    mkdir -p .cargo
    cp "../../.cargo/config.toml" .cargo/config.toml
  fi
  ```

- [ ] Execute this plan in the worktree using `superpowers:executing-plans`.
- [ ] Create a PR when implementation and local verification pass.
- [ ] Execute `lavra-review` in the worktree and address every issue it reports.
- [ ] Execute `pr-review-toolkit:full-review` in the worktree and address every issue it reports.
- [ ] Dispatch the `code_simplifier` agent to review every file touched in the PR and address every issue it reports.
- [ ] Execute `gh-address-comments` in the worktree and address every PR comment.

## Fixed Decisions

- [ ] Product/display/repo identity is `Hive` / `hive`.
- [ ] Cargo package name changes from `syslog-mcp` to `hive-mcp`.
- [ ] Rust import namespace changes from `syslog_mcp` to `hive_mcp`.
- [ ] Primary binary is `hive`.
- [ ] Keep a legacy `syslog` binary alias or wrapper when feasible.
- [ ] Primary public MCP tool is `hive`.
- [ ] Keep `syslog` as a compatibility MCP tool alias in this release.
- [ ] Primary MCP scopes are `hive:read` and `hive:admin`.
- [ ] Legacy scopes `syslog:read` and `syslog:admin` remain accepted during this release.
- [ ] Primary schema resource is `hive://schema/mcp-tool`.
- [ ] Legacy schema resource `syslog://schema/mcp-tool` remains readable during this release.
- [ ] Docker names migrate toward Hive naming, with explicit upgrade validation for preserved data behavior.
- [ ] `syslog` remains the correct protocol word for RFC syslog listeners, parsing, facilities, severities, ports, and log message semantics.

## Task 1: Lock the Rename Contract

- [ ] Claim bead `syslog-mcp-s4jl.1`.
- [ ] Add `docs/HIVE_REBRAND.md` with these sections:
  - `Identity Matrix`
  - `Compatibility Matrix`
  - `Data Preservation Contract`
  - `Auth and Scope Contract`
  - `Plugin Contract`
  - `Rollback Contract`
- [ ] In `Identity Matrix`, record these exact primary names:

  | Surface | Primary name |
  | --- | --- |
  | Product display | Hive |
  | Cargo package | hive-mcp |
  | Rust crate import | hive_mcp |
  | Primary binary | hive |
  | MCP tool | hive |
  | MCP read scope | hive:read |
  | MCP admin scope | hive:admin |
  | MCP schema resource | hive://schema/mcp-tool |
  | Plugin manifest name | hive |
  | Docker image | hive-mcp |

- [ ] In `Compatibility Matrix`, record each legacy name that remains accepted:

  | Legacy surface | Compatibility behavior |
  | --- | --- |
  | `syslog` binary | Runs the same server or prints an explicit migration message that invokes `hive` |
  | `syslog` MCP tool | Dispatches to the same action implementation as `hive` |
  | `syslog://schema/mcp-tool` | Returns the same schema resource as `hive://schema/mcp-tool` |
  | `syslog:read` / `syslog:admin` | Accepted alongside `hive:read` / `hive:admin` |
  | `SYSLOG_MCP_*` env vars | Accepted as legacy aliases with lower precedence than `HIVE_MCP_*` |
  | `SYSLOG_API_*` env vars | Accepted as legacy aliases with lower precedence than `HIVE_API_*` |
  | `SYSLOG_DOCKER_*` env vars | Accepted as legacy aliases with lower precedence than `HIVE_DOCKER_*` |
  | `SYSLOG_HOST` / `SYSLOG_PORT` | Remain canonical protocol listener settings |

- [ ] In `Data Preservation Contract`, state that no Compose migration may silently switch an existing operator from their current DB directory or named volume to an empty Hive volume.
- [ ] Add a review checklist to `docs/HIVE_REBRAND.md` that names all files touched by this plan.
- [ ] Run:

  ```bash
  rg -n "syslog-mcp|syslog_mcp|SYSLOG_MCP|syslog://|syslog:read|syslog:admin|name = \"syslog\"|\"name\": \"syslog\"" .
  ```

- [ ] Use the search output to update the file inventory in `docs/HIVE_REBRAND.md`.
- [ ] Close bead `syslog-mcp-s4jl.1` after the document includes the exact contract above and the search inventory.

## Task 2: Rename Cargo Package, Crate, and Binaries

- [ ] Claim bead `syslog-mcp-s4jl.2`.
- [ ] Update `Cargo.toml`:
  - package `name = "hive-mcp"`
  - package description uses `Hive MCP server` and keeps syslog receiver wording for the protocol
  - `[[bin]] name = "hive"` at `src/main.rs`
  - add a second `[[bin]] name = "syslog"` only if it can reuse `src/main.rs` without conflicting with CLI behavior
  - change the self dev-dependency key from `syslog-mcp` to `hive-mcp`
- [ ] If keeping a separate wrapper binary is cleaner than two `[[bin]]` entries pointing at `src/main.rs`, add `src/bin/syslog.rs` with a small compatibility entrypoint that delegates to the Hive server path and emits no misleading old branding.
- [ ] Update all Rust import references from `syslog_mcp` to `hive_mcp`.
- [ ] Update crate-level docs, CLI help text, and startup banners so product identity is Hive.
- [ ] Preserve protocol text such as `syslog listener`, `syslog parser`, `syslog port`, `RFC 3164`, and `RFC 5424`.
- [ ] Run:

  ```bash
  cargo check
  cargo test
  cargo run --bin hive -- --help
  cargo run --bin syslog -- --help
  ```

- [ ] If the legacy `syslog` binary is intentionally not provided, document that decision in `docs/HIVE_REBRAND.md` and remove the binary command above from the final verification list.
- [ ] Update `Cargo.lock` through Cargo, not by manual editing.
- [ ] Close bead `syslog-mcp-s4jl.2`.

## Task 3: Add Hive Config and Deployment Compatibility

- [ ] Claim bead `syslog-mcp-s4jl.3`.
- [ ] In `src/config.rs`, add Hive env aliases with this precedence:
  - `HIVE_MCP_*` overrides `SYSLOG_MCP_*`
  - `HIVE_API_*` overrides `SYSLOG_API_*`
  - `HIVE_DOCKER_*` overrides `SYSLOG_DOCKER_*`
  - `HIVE_*` generic app env vars override legacy app-level equivalents only where the value is not protocol-specific
  - `SYSLOG_HOST`, `SYSLOG_PORT`, `SYSLOG_MAX_MESSAGE_SIZE`, `SYSLOG_BATCH_SIZE`, and `SYSLOG_FLUSH_INTERVAL` remain valid protocol listener settings
- [ ] Keep `SYSLOG_MCP_TOKEN` accepted as a legacy alias for the MCP bearer token.
- [ ] Add `HIVE_MCP_TOKEN` as the primary bearer token env var.
- [ ] Make `SYSLOG_MCP_API_TOKEN` remain the deprecated fallback below both `HIVE_MCP_TOKEN` and `SYSLOG_MCP_TOKEN`.
- [ ] Update error messages so users configuring `HIVE_*` values see `HIVE_*` guidance first and legacy `SYSLOG_*` guidance only as compatibility text.
- [ ] Add config tests covering:
  - `HIVE_MCP_TOKEN` precedence over `SYSLOG_MCP_TOKEN`
  - `SYSLOG_MCP_TOKEN` precedence over deprecated `SYSLOG_MCP_API_TOKEN`
  - `HIVE_MCP_AUTH_MODE=oauth` startup validation names Hive vars in errors
  - `SYSLOG_HOST` / `SYSLOG_PORT` still configure the protocol listener
  - legacy `SYSLOG_DOCKER_HOSTS` still works
  - primary `HIVE_DOCKER_HOSTS` overrides legacy Docker hosts
- [ ] Update `docker-compose.yml`:
  - service name becomes `hive-mcp` or `hive`
  - image/container names become Hive branded
  - env uses primary `HIVE_MCP_*`, `HIVE_DOCKER_*`, and `HIVE_API_*` where applicable
  - syslog listener env remains protocol-accurate when the setting names are about RFC syslog
  - volume behavior keeps the existing data path visible and migration-safe
- [ ] Add `scripts/verify-compose-upgrade.sh` if no equivalent exists. It must prove an old deployment does not lose data by checking that an existing DB file remains the DB used by the Hive service.
- [ ] The compose upgrade script must:
  - create a temporary project directory
  - create a sentinel SQLite DB or sentinel file in the old data path
  - render old and new Compose config
  - start the new Compose stack against the preserved path when Docker is available
  - fail if the new stack points at a different empty data location
  - skip live container startup with an explicit message when Docker is unavailable, while still validating rendered paths
- [ ] Run:

  ```bash
  cargo test config
  docker compose config
  bash scripts/verify-compose-upgrade.sh
  ```

- [ ] Close bead `syslog-mcp-s4jl.3`.

## Task 4: Rebrand MCP and Plugin Surfaces

- [ ] Claim bead `syslog-mcp-s4jl.4`.
- [ ] In `src/mcp/schemas.rs`, change tool definitions so `hive` is the primary listed tool.
- [ ] Keep a legacy `syslog` dispatch path in the MCP server even if `syslog` is not listed as the preferred tool.
- [ ] Implement explicit constants in the MCP layer for primary and legacy names:

  ```rust
  const HIVE_TOOL_NAME: &str = "hive";
  const LEGACY_SYSLOG_TOOL_NAME: &str = "syslog";
  const HIVE_READ_SCOPE: &str = "hive:read";
  const LEGACY_SYSLOG_READ_SCOPE: &str = "syslog:read";
  const HIVE_ADMIN_SCOPE: &str = "hive:admin";
  const LEGACY_SYSLOG_ADMIN_SCOPE: &str = "syslog:admin";
  const HIVE_SCHEMA_RESOURCE_URI: &str = "hive://schema/mcp-tool";
  const LEGACY_SYSLOG_SCHEMA_RESOURCE_URI: &str = "syslog://schema/mcp-tool";
  ```

- [ ] Update scope mapping so every public read-only action accepts `hive:read` and legacy `syslog:read`.
- [ ] Update admin or write actions so they accept `hive:admin` and legacy `syslog:admin`.
- [ ] Add MCP tests covering:
  - `tools/list` includes `hive`
  - `tools/call` accepts `hive`
  - `tools/call` accepts legacy `syslog`
  - `resources/read` accepts `hive://schema/mcp-tool`
  - `resources/read` accepts legacy `syslog://schema/mcp-tool`
  - OAuth bearer with `hive:read` can call all public read-only actions
  - OAuth bearer with only `syslog:read` can call all public read-only actions
  - mounted auth coverage still covers every MCP action
  - alias error messages mention Hive first
- [ ] Update `.claude-plugin/plugin.json`:
  - `"name": "hive"`
  - description uses Hive wording
  - repository uses the Hive repository URL after the GitHub repo exists
  - keywords include `hive`, `mcp`, `logging`, `observability`, `syslog`
  - user-facing titles and descriptions say Hive for the service name
  - protocol-specific settings continue to say syslog when they describe the syslog receiver
  - all token, secret, password, credential, and private-key-like fields are marked `"sensitive": true`
- [ ] Update `.codex-plugin/plugin.json` if present with the same manifest rules.
- [ ] Update plugin server files under `.claude-plugin/plugins/` and any mirrored Codex plugin files so MCP server names, command names, env vars, and generated setup output use Hive.
- [ ] Keep command compatibility where practical:
  - primary slash commands use `/hive:*`
  - legacy `/syslog:*` commands remain wrappers or documented aliases if plugin command compatibility supports that shape
- [ ] Run:

  ```bash
  cargo test mcp
  bash scripts/smoke-test.sh
  jq . .claude-plugin/plugin.json >/dev/null
  [ ! -f .codex-plugin/plugin.json ] || jq . .codex-plugin/plugin.json >/dev/null
  ```

- [ ] Close bead `syslog-mcp-s4jl.4`.

## Task 5: Update Docs, Metadata, Scripts, and Migration Notes

- [ ] Claim bead `syslog-mcp-s4jl.5`.
- [ ] Update `README.md` so the project title and first viewport identity are Hive.
- [ ] Update all docs under `docs/` for Hive naming while preserving syslog protocol language.
- [ ] Add a migration section that includes:
  - old-to-new env var mapping
  - old-to-new binary command mapping
  - MCP tool/resource/scope compatibility
  - Docker Compose data preservation requirements
  - rollback command notes
- [ ] Update `CHANGELOG.md` with an unreleased Hive rebrand entry or the next version entry selected by the release script.
- [ ] Update `scripts/bump-version.sh` and `scripts/check-version-sync.sh` so `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`, Cargo, and docs versions stay aligned after the package rename.
- [ ] Update CI and release workflows under `.github/workflows/` for `hive-mcp` package and artifact names.
- [ ] Update Docker publishing metadata and image references in:
  - `Dockerfile`
  - `docker-compose.yml`
  - Compose examples under `docs/`
  - release scripts
  - plugin setup hooks
- [ ] Run these searches and classify every remaining hit:

  ```bash
  rg -n "syslog-mcp|syslog_mcp|SYSLOG_MCP|SYSLOG_API|SYSLOG_DOCKER|syslog://|syslog:read|syslog:admin|/syslog:|name = \"syslog\"|\"name\": \"syslog\"" .
  rg -n "Hive|hive-mcp|hive_mcp|HIVE_MCP|HIVE_API|HIVE_DOCKER|hive://|hive:read|hive:admin" .
  ```

- [ ] For each remaining legacy `syslog` hit, add a short classification in `docs/HIVE_REBRAND.md`:
  - protocol term
  - legacy compatibility alias
  - migration example
  - intentionally unchanged external reference
- [ ] Run:

  ```bash
  cargo fmt --check
  cargo clippy --all-targets --all-features -- -D warnings
  cargo test
  bash scripts/check-version-sync.sh
  ```

- [ ] Close bead `syslog-mcp-s4jl.5`.

## Task 6: Final Verification, PR, and Review Closure

- [ ] Claim bead `syslog-mcp-s4jl.6`.
- [ ] Run the full local verification gate:

  ```bash
  cargo fmt --check
  cargo clippy --all-targets --all-features -- -D warnings
  cargo test
  docker compose config
  bash scripts/verify-compose-upgrade.sh
  bash scripts/check-version-sync.sh
  bash scripts/smoke-test.sh
  ```

- [ ] If `scripts/smoke-test.sh` requires a live server, start the Hive server with the primary binary and run smoke tests against it:

  ```bash
  cargo run --bin hive -- serve mcp
  ```

- [ ] Stop any server started for verification before committing.
- [ ] Confirm Beads state:

  ```bash
  bd ready --json
  bd show syslog-mcp-s4jl --json
  bd children syslog-mcp-s4jl --json
  ```

- [ ] Close every completed child bead.
- [ ] Commit all implementation changes with a message that reflects the release scale:

  ```bash
  git status --short
  git add .
  git commit -m "feat!: rebrand syslog-mcp to Hive"
  bd dolt commit -m "close Hive rebrand implementation"
  ```

- [ ] Push the branch and Beads state:

  ```bash
  git push -u origin hive-rebrand
  bd dolt push
  ```

- [ ] Create a PR:

  ```bash
  gh pr create --fill --base main --head hive-rebrand
  ```

- [ ] Execute `lavra-review` in the worktree.
- [ ] Convert every actionable `lavra-review` finding into code/docs/tests changes in the same worktree.
- [ ] Re-run the targeted verification for every file area changed by `lavra-review`.
- [ ] Execute `pr-review-toolkit:full-review` in the worktree.
- [ ] Convert every actionable full-review finding into code/docs/tests changes in the same worktree.
- [ ] Dispatch the `code_simplifier` agent with this scope:

  ```text
  Review every file touched by the Hive rebrand PR. Preserve behavior and compatibility contracts. Report concrete simplification opportunities and risks; do not make speculative rewrites.
  ```

- [ ] Address every concrete `code_simplifier` issue in the worktree.
- [ ] Execute `gh-address-comments` in the worktree.
- [ ] Address every PR comment with code/docs/tests changes or a clear GitHub reply when no change is warranted.
- [ ] Re-run the full verification gate after review changes.
- [ ] Push final changes:

  ```bash
  git status --short
  git add .
  git commit -m "fix: address Hive rebrand review feedback"
  bd dolt commit -m "address Hive rebrand review feedback"
  git push
  bd dolt push
  ```

- [ ] Close bead `syslog-mcp-s4jl.6`.
- [ ] Close epic `syslog-mcp-s4jl` only after all children are closed, the PR is green, and `gh-address-comments` reports no unresolved actionable comments.

## Final Acceptance Criteria

- [ ] `cargo metadata --no-deps` reports package `hive-mcp`.
- [ ] `cargo run --bin hive -- --help` works.
- [ ] Legacy `syslog` binary behavior is either working or explicitly documented as not retained.
- [ ] MCP `tools/list` exposes `hive` as the primary tool.
- [ ] MCP `tools/call` accepts both `hive` and legacy `syslog`.
- [ ] MCP resource reads accept both `hive://schema/mcp-tool` and legacy `syslog://schema/mcp-tool`.
- [ ] OAuth read-only tokens with `hive:read` can call all public read-only actions.
- [ ] OAuth read-only tokens with legacy `syslog:read` can call all public read-only actions.
- [ ] `.claude-plugin/plugin.json` is Hive-branded and validates as JSON.
- [ ] Every sensitive token/secret/password-like plugin config field has `"sensitive": true`.
- [ ] Docker Compose validates with Hive names.
- [ ] Compose upgrade verification proves preserved data path behavior.
- [ ] Remaining `syslog` references are classified as protocol, legacy compatibility, migration example, or intentional external reference.
- [ ] All review tools named in the execution contract have been run and their actionable findings addressed.
