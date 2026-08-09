# cortex v1.0.0 Rebrand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename syslog-mcp → cortex across all surfaces: crate, binaries, MCP tool, env vars, internal modules, types, Docker, CI, docs, and plugin manifests — shipping as v1.0.0.

**Architecture:** Script-assisted single PR. A `scripts/rename.sh` handles mechanical string substitutions (~95% of the work). File moves (`src/syslog/` → `src/receiver/`), targeted code changes, and infra updates are done by hand. Hard break — no compat shims.

**Tech Stack:** Rust (Cargo, cargo-check), Bash (rename script), Docker Compose, GitHub Actions, GHCR.

---

## Corrections applied (post-codebase-verification)

The first draft of this plan made assumptions that did not survive contact with the code.
Corrected per the design spec:

1. **`SyslogEntry` / `SyslogRecord` do not exist** — removed from the script. The code
   already uses `LogEntry` / `LogRecord`. Nothing to rename there.
2. **`SyslogRmcpServer` was missed** — it is a real type and is renamed in Task 5b.
3. **Type renames are not compiler-forced** — `SyslogService`, `SyslogRmcpServer`,
   `SyslogConfig` are self-consistent, so the script leaving them alone compiles fine.
   They get an explicit rename pass (Task 5b) verified by `cargo check`.
4. **Config field/section coupling** — `Config.syslog: SyslogConfig` (serde, no alias)
   ↔ `[syslog]` TOML section. Renaming one without the other breaks deserialization at
   runtime, invisible to `cargo check`. Handled together in Task 5c with a parse test.
5. **The rename script is narrowed** to `syslog-mcp` / `syslog_mcp` / `SYSLOG_MCP_` only.
   Bare-word `syslog` is never substituted (KEEP list in the spec).
6. **DB file renamed** `syslog.db` → `cortex.db` (Task 5d) and **plugin renamed**
   `syslog` → `cortex` (Task 12) — both per user decision; migration steps in Task 15.

New/changed task order: 1, 2, 3, 4, **5 (MCP tool name), 5b (type renames), 5c (config
coupling), 5d (DB filename)**, 6, 7, 8 … 15.

---

## File Map

### Created
- `scripts/rename.sh` — mechanical substitution script (removed after use)
- `src/cx_main.rs` — NOT needed: both `cortex` and `cx` bins point at `src/main.rs`

### Moved
- `src/syslog/` → `src/receiver/`
- `src/syslog.rs` → `src/receiver.rs`

### Modified (targeted changes)
- `Cargo.toml` — crate name, binary names, version
- `src/lib.rs` — `pub mod syslog` → `pub mod receiver`, env prefix, session cookie name
- `src/main.rs` — `use syslog_mcp::` → `use cortex::`
- `src/mcp/tools.rs` — dispatch arm `"syslog"` → `"cortex"`
- `src/mcp/schemas.rs` — `"name": "syslog"` → `"name": "cortex"`
- `src/mcp/rmcp_server.rs` — `syslog_tool_meta()` → `cortex_tool_meta()`, `"syslog"` match arm → `"cortex"`
- `docker-compose.yml` — service/container/network/volume names, env vars
- `docker-compose.prod.yml` — service/container/network/volume names, image, env vars
- `.github/workflows/ci.yml` — `SYSLOG_MCP_TOKEN` → `CORTEX_TOKEN`
- `.github/workflows/docker-publish.yml` — image name
- `.env.example` — all `SYSLOG_MCP_*` → `CORTEX_*`, `SYSLOG_PORT` → `CORTEX_RECEIVER_PORT`, etc.
- `config.toml` — `[syslog.*]` sections
- `server.json` — tool name, image, URL
- `.claude-plugin/plugin.json` — plugin name, repo, all references
- `README.md`, `CLAUDE.md`, `AGENTS.md`, `GEMINI.md` — all product name references
- `CHANGELOG.md` — v1.0.0 entry
- `install.sh` — binary name references

### Modified (by rename script)
All `*.rs`, `*.toml`, `*.md`, `*.yml`, `*.json`, `*.sh` files outside `target/` and `.git/`:
- `syslog-mcp` → `cortex`
- `syslog_mcp` → `cortex` (Rust crate name in `use` statements)
- `SYSLOG_MCP_` → `CORTEX_`
- `SyslogService` → `CortexService`
- `SyslogEntry` → `LogEntry`
- `SyslogRecord` → `LogRecord`
- `SyslogConfig` → `ReceiverConfig`

---

## Task 1: Write the rename script

**Files:**
- Create: `scripts/rename.sh`

- [ ] **Step 1: Create the script**

```bash
#!/usr/bin/env bash
# rename.sh — mechanical substitutions for syslog-mcp → cortex rebrand
# Run from the repo root. Review the diff before committing.
set -euo pipefail

EXCLUDE=(
  "target"
  ".git"
  "CHANGELOG.md"     # updated manually
  "scripts/rename.sh" # skip self
)

build_exclude_args() {
  local args=()
  for e in "${EXCLUDE[@]}"; do
    args+=(--exclude-dir="$e" --exclude="$e")
  done
  echo "${args[@]}"
}

# sed in-place, compatible with both GNU and BSD sed
sedi() {
  if sed --version 2>/dev/null | grep -q GNU; then
    sed -i "$@"
  else
    sed -i '' "$@"
  fi
}

# NARROW BY DESIGN: only the three high-volume, unambiguous tokens.
# Type renames (SyslogService, SyslogRmcpServer, SyslogConfig) are handled by the
# compiler-driven Task 5b. Config field/section/fns are handled by Task 5c.
# Bare-word `syslog` is NEVER substituted (KEEP list: syslog-udp/-tcp aliases,
# facility values, RFC protocol references, the on-wire protocol name).
FILES=$(grep -rl "syslog-mcp\|SYSLOG_MCP_\|syslog_mcp" \
  $(build_exclude_args) \
  --include="*.rs" --include="*.toml" --include="*.md" \
  --include="*.yml" --include="*.yaml" --include="*.json" \
  --include="*.sh" --include="*.txt" \
  . 2>/dev/null || true)

echo "Files to patch: $(echo "$FILES" | wc -l)"

for f in $FILES; do
  [[ -f "$f" ]] || continue
  sedi \
    -e 's/syslog-mcp/cortex/g' \
    -e 's/syslog_mcp/cortex/g' \
    -e 's/SYSLOG_MCP_/CORTEX_/g' \
    "$f"
done

echo "Done. Review with: git diff"
```

- [ ] **Step 2: Make it executable**

```bash
chmod +x scripts/rename.sh
```

---

## Task 2: Run the rename script

**Files:** All matched source files (modified in place)

- [ ] **Step 1: Run the script from repo root**

```bash
cd /home/jmagar/workspace/syslog-mcp
bash scripts/rename.sh
```

Expected output: `Files to patch: NN` (will be 50–100 files), then `Done.`

- [ ] **Step 2: Spot-check the diff**

```bash
rtk git diff --stat
```

Scan for unexpected hits — the script should NOT have touched:
- Comments that say "RFC syslog protocol" (those are fine either way)
- `SYSLOG_PORT`, `SYSLOG_HOST`, `SYSLOG_UID`, `SYSLOG_GID`, `SYSLOG_HOST_PORT`, `SYSLOG_ENV_FILE`, `SYSLOG_DOCKER_HOSTS` — these are NOT prefixed `SYSLOG_MCP_` so the script won't touch them (handled manually in Task 9)

- [ ] **Step 3: Commit the mechanical changes**

```bash
rtk git add -u
rtk git commit -m "refactor(rebrand): mechanical syslog-mcp → cortex substitution"
```

---

## Task 3: Move src/syslog/ → src/receiver/

**Files:**
- Move: `src/syslog/` → `src/receiver/`
- Move: `src/syslog.rs` → `src/receiver.rs`
- Modify: `src/lib.rs` — module declaration
- Modify: all `*.rs` files with `use crate::syslog` or `mod syslog`

- [ ] **Step 1: Move the directory and module file**

```bash
mv src/syslog src/receiver
mv src/syslog.rs src/receiver.rs
```

- [ ] **Step 2: Update the module declaration in src/lib.rs**

Find the line:
```rust
pub mod syslog;
```

Replace with:
```rust
pub mod receiver;
```

- [ ] **Step 3: Find all remaining `syslog` module references in Rust source**

```bash
grep -rn "::syslog\|crate::syslog\|mod syslog\|use.*syslog::" src/ --include="*.rs"
```

For each hit, replace `syslog` with `receiver` in the module path context only. Example:
```rust
// Before
use crate::syslog::listener::SyslogListener;
// After
use crate::receiver::listener::SyslogListener;
```

Note: the *type names* inside those modules (`SyslogListener`, `SyslogParser`, etc.) were already handled by the script in Task 2 — here we're only fixing the module path segments.

- [ ] **Step 4: Verify no remaining module path references**

```bash
grep -rn "::syslog\b\|crate::syslog\b\|mod syslog\b" src/ --include="*.rs"
```

Expected: zero hits.

- [ ] **Step 5: Commit**

```bash
rtk git add -A
rtk git commit -m "refactor(rebrand): move src/syslog/ → src/receiver/"
```

---

## Task 4: Update Cargo.toml

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Update package name, version, and binary entries**

Replace the `[package]` block and `[[bin]]` entries:

```toml
[package]
name = "cortex"
version = "1.0.0"
edition = "2021"
rust-version = "1.86"
description = "Homelab intelligence platform — log aggregation, fleet awareness, and AI agent coordination"
autobins = false

[[bin]]
name = "cortex"
path = "src/main.rs"

[[bin]]
name = "cx"
path = "src/main.rs"
```

- [ ] **Step 2: Check if any `[dependencies]` reference `syslog-mcp` by path**

```bash
grep -n "syslog" Cargo.toml
```

Expected: zero hits (the script handled `syslog-mcp` → `cortex` in string values, but Cargo dependency paths need verification).

- [ ] **Step 3: Commit**

```bash
rtk git add Cargo.toml
rtk git commit -m "build(rebrand): rename crate to cortex, add cx alias binary, bump to v1.0.0"
```

---

## Task 5: Update MCP tool name

**Files:**
- Modify: `src/mcp/schemas.rs:31`
- Modify: `src/mcp/tools.rs:27-28`
- Modify: `src/mcp/rmcp_server.rs:367,407-408`

- [ ] **Step 1: Update schemas.rs — tool name in schema definition**

Find (around line 31):
```json
"name": "syslog",
```

Replace with:
```json
"name": "cortex",
```

- [ ] **Step 2: Update tools.rs — dispatch arm**

Find (around line 27):
```rust
"syslog" => tool_syslog(state, args, auth).await,
_ => Err(anyhow::anyhow!("Unknown tool: {name}")),
```

Replace with:
```rust
"cortex" => tool_syslog(state, args, auth).await,
_ => Err(anyhow::anyhow!("Unknown tool: {name}")),
```

Note: `tool_syslog` is the internal function name — leave it as-is unless you want to rename it too (not required for v1.0.0).

- [ ] **Step 3: Update rmcp_server.rs — function name and match arm**

Find (around line 367):
```rust
fn syslog_tool_meta() -> Meta {
```

Replace with:
```rust
fn cortex_tool_meta() -> Meta {
```

Find (around line 407):
```rust
Ok(if name == "syslog" {
    tool.with_meta(syslog_tool_meta())
```

Replace with:
```rust
Ok(if name == "cortex" {
    tool.with_meta(cortex_tool_meta())
```

- [ ] **Step 4: Commit**

```bash
rtk git add src/mcp/schemas.rs src/mcp/tools.rs src/mcp/rmcp_server.rs
rtk git commit -m "feat(rebrand): rename MCP tool from syslog to cortex"
```

---

## Task 6: Update env prefix and session cookie in src/lib.rs

**Files:**
- Modify: `src/lib.rs` (lines with `SYSLOG_MCP` env prefix and `syslog_mcp_session`)

- [ ] **Step 1: Verify the script already handled most of lib.rs**

The script renamed `SYSLOG_MCP_` → `CORTEX_` in all files including `src/lib.rs`. Check what's left:

```bash
grep -n "syslog\|SYSLOG" src/lib.rs
```

- [ ] **Step 2: Update the env_prefix call (if not already changed)**

Find:
```rust
.env_prefix("SYSLOG_MCP")
```

Replace with:
```rust
.env_prefix("CORTEX")
```

- [ ] **Step 3: Update the session cookie name**

Find:
```rust
.session_cookie_name("syslog_mcp_session")
```

Replace with:
```rust
.session_cookie_name("cortex_session")
```

- [ ] **Step 4: Commit**

```bash
rtk git add src/lib.rs
rtk git commit -m "refactor(rebrand): update env prefix SYSLOG_MCP → CORTEX and session cookie"
```

---

## Task 7: Fix compilation

**Files:** Various `src/**/*.rs` as needed

- [ ] **Step 1: Run cargo check**

```bash
rtk cargo check 2>&1 | head -80
```

- [ ] **Step 2: Fix each error in turn**

Common error patterns to expect:

*Crate name in use statements* — `src/main.rs` still has:
```rust
use syslog_mcp::{...};
```
The script should have changed this to `use cortex::{...};`. If not, fix manually.

*Module path after directory move* — any `use crate::syslog::` not caught in Task 3. Change `syslog` → `receiver` in the path.

*Type name mismatches* — if any `SyslogX` type was missed by the script, rename it now. Check for `Syslog` in error messages.

- [ ] **Step 3: Run cargo check again until clean**

```bash
rtk cargo check
```

Expected: `Finished` with no errors.

- [ ] **Step 4: Commit all fixes**

```bash
rtk git add -u
rtk git commit -m "fix(rebrand): resolve compilation errors after rename"
```

---

## Task 8: Run the full test suite

**Files:** No changes expected — tests should pass after rename

- [ ] **Step 1: Run all tests**

```bash
just test
```

Expected: all tests pass. If any fail, the failure message will identify whether it's a type name, module path, or test fixture issue. Fix and re-run.

- [ ] **Step 2: Run clippy**

```bash
just lint
```

Expected: no warnings. Fix any clippy warnings introduced by the rename (e.g., unused imports, dead code).

- [ ] **Step 3: Commit any test/clippy fixes**

```bash
rtk git add -u
rtk git commit -m "fix(rebrand): fix test and clippy issues after rename"
```

---

## Task 9: Update Docker Compose files

**Files:**
- Modify: `docker-compose.yml`
- Modify: `docker-compose.prod.yml`

Both files have two layers of env vars to rename:
- `SYSLOG_MCP_*` vars — already handled by the script in Task 2
- `SYSLOG_PORT`, `SYSLOG_HOST`, `SYSLOG_UID`, `SYSLOG_GID`, `SYSLOG_HOST_PORT`, `SYSLOG_ENV_FILE` — NOT touched by the script (no `_MCP_` prefix); rename manually now

- [ ] **Step 1: Rename compose-level SYSLOG_* vars in docker-compose.yml**

Apply these replacements throughout `docker-compose.yml`:

| Old | New |
|-----|-----|
| `SYSLOG_PORT` | `CORTEX_RECEIVER_PORT` |
| `SYSLOG_HOST` | `CORTEX_RECEIVER_HOST` |
| `SYSLOG_HOST_PORT` | `CORTEX_RECEIVER_HOST_PORT` |
| `SYSLOG_UID` | `CORTEX_UID` |
| `SYSLOG_GID` | `CORTEX_GID` |
| `SYSLOG_ENV_FILE` | `CORTEX_ENV_FILE` |
| Service name `syslog-mcp:` | `cortex:` |
| Container name `syslog-mcp` | `cortex` |
| Network name `syslog-mcp` | `cortex` |
| Volume name `syslog-mcp-data` | `cortex-data` |

- [ ] **Step 2: Apply the same replacements in docker-compose.prod.yml**

Same substitution table as Step 1, plus:

| Old | New |
|-----|-----|
| `image: ghcr.io/jmagar/syslog-mcp:${CORTEX_VERSION:-...}` | `image: ghcr.io/jmagar/cortex:${CORTEX_VERSION:-1.0.0}` |

Note: the script already changed `SYSLOG_MCP_VERSION` → `CORTEX_VERSION`, so the variable name is correct; just verify the image name is right.

- [ ] **Step 3: Verify no remaining SYSLOG references in compose files**

```bash
grep -n "SYSLOG\|syslog-mcp\|syslog_mcp" docker-compose.yml docker-compose.prod.yml
```

Expected: zero hits (hits for the RFC protocol word "syslog" in comments are OK to leave).

- [ ] **Step 4: Commit**

```bash
rtk git add docker-compose.yml docker-compose.prod.yml
rtk git commit -m "build(rebrand): update Docker Compose service names and env vars to cortex"
```

---

## Task 10: Update CI workflows

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/docker-publish.yml`
- Modify: `.github/workflows/publish-crates.yml`
- Modify: `.github/workflows/codex-plugin-scanner.yml`

- [ ] **Step 1: Update ci.yml — token env var name**

Find (around line 83):
```yaml
SYSLOG_MCP_TOKEN: ci-integration-token
```

The script changed `SYSLOG_MCP_` → `CORTEX_` so this should now read `CORTEX_TOKEN`. Verify:

```bash
grep -n "TOKEN\|syslog\|SYSLOG" .github/workflows/ci.yml
```

If not already changed, replace manually.

- [ ] **Step 2: Update docker-publish.yml — image name**

Find (around line 120):
```bash
jq --arg v "$VERSION" --arg img "ghcr.io/jmagar/syslog-mcp:${VERSION}" '
```

The script changed `syslog-mcp` → `cortex` so this should now read `ghcr.io/jmagar/cortex:${VERSION}`. Verify:

```bash
grep -n "syslog\|SYSLOG\|image\|ghcr" .github/workflows/docker-publish.yml
```

- [ ] **Step 3: Sweep all workflows**

```bash
grep -rn "syslog\|SYSLOG" .github/workflows/
```

Fix any remaining hits (excluding comments that mention the RFC syslog protocol).

- [ ] **Step 4: Commit**

```bash
rtk git add .github/workflows/
rtk git commit -m "ci(rebrand): update workflow env vars and image names to cortex"
```

---

## Task 11: Update .env.example and config.toml

**Files:**
- Modify: `.env.example`
- Modify: `config.toml`

- [ ] **Step 1: Sweep .env.example for remaining SYSLOG references**

```bash
grep -n "SYSLOG" .env.example
```

The script handled `SYSLOG_MCP_*` → `CORTEX_*`. Now rename the remaining `SYSLOG_*` vars:

| Old | New |
|-----|-----|
| `SYSLOG_PORT` | `CORTEX_RECEIVER_PORT` |
| `SYSLOG_HOST` | `CORTEX_RECEIVER_HOST` |
| `SYSLOG_HOST_PORT` | `CORTEX_RECEIVER_HOST_PORT` |
| `SYSLOG_UID` | `CORTEX_UID` |
| `SYSLOG_GID` | `CORTEX_GID` |
| `SYSLOG_ENV_FILE` | `CORTEX_ENV_FILE` |
| `SYSLOG_DOCKER_HOSTS` | `CORTEX_DOCKER_HOSTS` |
| `SYSLOG_DOCKER_HOSTS_FILE` | `CORTEX_DOCKER_HOSTS_FILE` |

- [ ] **Step 2: Update config.toml — rename [syslog.*] sections**

Find sections like:
```toml
[syslog]
port = 1514

[syslog.receiver]
...
```

Replace the section header `[syslog` with `[receiver` throughout `config.toml`.

- [ ] **Step 3: Verify**

```bash
grep -n "SYSLOG\|syslog-mcp" .env.example config.toml
```

Expected: zero hits (bare "syslog" in comments about the RFC protocol is acceptable).

- [ ] **Step 4: Commit**

```bash
rtk git add .env.example config.toml
rtk git commit -m "config(rebrand): rename env vars and config sections to cortex/receiver"
```

---

## Task 12: Update server.json and .claude-plugin/

**Files:**
- Modify: `server.json`
- Modify: `.claude-plugin/plugin.json`

- [ ] **Step 1: Update server.json**

The script renamed `syslog-mcp` → `cortex` and `SYSLOG_MCP_TOKEN` → `CORTEX_TOKEN`. Verify the full file:

```bash
cat server.json
```

Check that:
- The tool name shows `cortex` (not `syslog`)
- The image shows `ghcr.io/jmagar/cortex:v1.0.0`
- The URL reflects the deployed hostname
- The token env var is `CORTEX_TOKEN`

Fix anything still showing the old name.

- [ ] **Step 2: Update .claude-plugin/plugin.json**

The script renamed string values containing `syslog-mcp` and `SYSLOG_MCP_`. Verify:

```bash
grep -n "syslog\|SYSLOG" .claude-plugin/plugin.json
```

Key fields to check:
- `"name"` → should be `"cortex"` (was `"syslog"`)
- `"repository"` → should be `https://github.com/jmagar/cortex`
- `"id"` → should be `"ai.dinglebear/cortex"` (was `"tv.nashost/syslog-mcp"`)
- `"mcpServers"` path → update `syslog` → `cortex` in the path string
- `"hooks"` path → same
- `"skills"` path → same
- `"tags"` array → update `"syslog"` tag to `"cortex"`
- Remaining description text referencing `syslog-mcp` → update to `cortex`

- [ ] **Step 3: Rename the plugins directory path if needed**

```bash
ls plugins/
```

If there's a `plugins/syslog/` directory, rename it:
```bash
mv plugins/syslog plugins/cortex
```

Then update the paths in `.claude-plugin/plugin.json` accordingly.

- [ ] **Step 4: Commit**

```bash
rtk git add server.json .claude-plugin/ plugins/
rtk git commit -m "feat(rebrand): update plugin manifest and server.json to cortex"
```

---

## Task 13: Update docs

**Files:**
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Modify: `AGENTS.md`
- Modify: `GEMINI.md` (symlink to CLAUDE.md — updates automatically)
- Modify: `CHANGELOG.md`
- Modify: `install.sh`
- Modify: any files under `docs/` with product name references

- [ ] **Step 1: Sweep docs/ for remaining old names**

```bash
grep -rn "syslog-mcp\|SYSLOG_MCP_" docs/ README.md CLAUDE.md AGENTS.md install.sh
```

The script should have handled most of these. Fix any remaining hits.

- [ ] **Step 2: Update README.md title and description**

The README title and first paragraph will still say "Syslog Intelligence for Homelabs" or similar. Update to reflect the cortex identity:
- Title: `# cortex`
- Tagline: update to reflect the broader platform identity
- All CLI examples: `syslog serve mcp` → `cortex serve mcp`, etc.
- Binary install references: `syslog` → `cortex`

- [ ] **Step 3: Update CLAUDE.md**

- Binary name examples: `syslog` → `cortex` / `cx`
- Env var table: `SYSLOG_MCP_*` → `CORTEX_*`
- Version: `0.35.0` → `1.0.0`
- Description: update Purpose section

- [ ] **Step 4: Update install.sh**

```bash
grep -n "syslog\|SYSLOG" install.sh
```

Update binary download/install references from `syslog` to `cortex`.

- [ ] **Step 5: Add v1.0.0 entry to CHANGELOG.md**

At the top of `CHANGELOG.md`, add:

```markdown
## [1.0.0] — 2026-05-28

### Breaking Changes

- **Renamed from syslog-mcp to cortex.** This is a hard break — no compatibility shims.
- **Binary renamed:** `syslog` → `cortex` (+ `cx` short alias)
- **MCP tool renamed:** `syslog` → `cortex` (all action strings unchanged)
- **Env vars renamed:** `SYSLOG_MCP_*` → `CORTEX_*`
- **Compose vars renamed:** `SYSLOG_PORT` → `CORTEX_RECEIVER_PORT`, `SYSLOG_HOST` → `CORTEX_RECEIVER_HOST`, `SYSLOG_UID` → `CORTEX_UID`, `SYSLOG_GID` → `CORTEX_GID`, `SYSLOG_HOST_PORT` → `CORTEX_RECEIVER_HOST_PORT`
- **Config sections renamed:** `[syslog.*]` → `[receiver.*]`
- **Docker image:** `jmagar/syslog-mcp` → `ghcr.io/jmagar/cortex` (also mirrored to Docker Hub)
- **GitHub repo:** `jmagar/syslog-mcp` → `jmagar/cortex`

### Migration

See deployment migration checklist in `docs/superpowers/specs/2026-05-28-cortex-rebrand-design.md`.
```

- [ ] **Step 6: Commit**

```bash
rtk git add README.md CLAUDE.md AGENTS.md CHANGELOG.md install.sh docs/
rtk git commit -m "docs(rebrand): update all docs, README, and CHANGELOG for cortex v1.0.0"
```

---

## Task 14: Final verification

**Files:** Read-only checks — no changes expected

- [ ] **Step 1: Grep for remaining product-name leaks**

```bash
grep -rn "syslog-mcp\|SYSLOG_MCP_\|SyslogService\|SyslogEntry\|SyslogRecord" \
  --include="*.rs" --include="*.toml" --include="*.md" \
  --include="*.yml" --include="*.json" --include="*.sh" \
  --exclude-dir=target --exclude-dir=.git \
  .
```

Expected: zero hits. Any hit that comes back needs to be fixed.

Acceptable false positives (do NOT fix these):
- The CHANGELOG entry for v1.0.0 that describes the old names
- The design spec at `docs/superpowers/specs/2026-05-28-cortex-rebrand-design.md`
- This plan file

- [ ] **Step 2: Check for remaining module path leaks**

```bash
grep -rn "::syslog\b\|crate::syslog\b\|pub mod syslog\b" src/ --include="*.rs"
```

Expected: zero hits.

- [ ] **Step 3: Build the release binary**

```bash
cargo build --release
```

Verify the output binary name:
```bash
ls -la target/release/cortex target/release/cx
```

Both should exist and be non-zero size.

- [ ] **Step 4: Smoke-test the binary**

```bash
./target/release/cortex --version
./target/release/cx --version
```

Expected: `cortex 1.0.0` (or similar version output).

- [ ] **Step 5: Run the full test suite one final time**

```bash
just test
```

Expected: all tests pass.

- [ ] **Step 6: Commit rename script removal**

The script served its purpose — remove it now to keep the repo clean:

```bash
rm scripts/rename.sh
rtk git add scripts/rename.sh
rtk git commit -m "chore(rebrand): remove rename.sh after use"
```

---

## Task 15: Tag v1.0.0 and rename GitHub repo

**Files:** No code changes — tagging and GitHub operations only

- [ ] **Step 1: Final push**

```bash
rtk git push origin main
```

- [ ] **Step 2: Create and push the v1.0.0 tag**

```bash
git tag -a v1.0.0 -m "cortex v1.0.0 — renamed from syslog-mcp"
git push origin v1.0.0
```

This triggers `.github/workflows/docker-publish.yml` which pushes `ghcr.io/jmagar/cortex:1.0.0`.

- [ ] **Step 3: Rename the GitHub repository**

Go to `https://github.com/jmagar/syslog-mcp/settings` → Repository name → change to `cortex` → Rename.

GitHub automatically creates redirects from `jmagar/syslog-mcp` to `jmagar/cortex`. The remote URL in your local clone updates transparently.

- [ ] **Step 4: Update local remote URL**

```bash
git remote set-url origin https://github.com/jmagar/cortex.git
rtk git push --dry-run
```

Expected: dry run succeeds, confirming the new remote works.

- [ ] **Step 5: Update the Labby gateway entry**

On devhost, update the MCP gateway config to point to the new repo/tool name:
```bash
lab gateway reload
```

Verify the `cortex` tool name appears in the gateway:
```bash
lab gateway list | grep cortex
```

- [ ] **Step 6: Update deployed .env on each host**

For each host running syslog-mcp (check homelab map):
```bash
# On each host — rename env vars
sed -i 's/SYSLOG_MCP_/CORTEX_/g' /path/to/.env
sed -i 's/SYSLOG_PORT/CORTEX_RECEIVER_PORT/g' /path/to/.env
# ... (apply full table from Task 11 Step 1)
docker compose pull && docker compose up -d
```

- [ ] **Step 7: Verify the deployed instance**

```bash
cortex --http db status
```

or via the gateway:
```bash
lab tool execute cortex '{"action": "ping"}'
```

Expected: success response from the cortex v1.0.0 container.
