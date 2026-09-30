# MCPB Package Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a reproducible Linux MCPB bundle for the existing `syslog mcp` stdio server.

**Architecture:** Add a source `mcpb/manifest.json` that describes the bundled Rust binary as a local stdio MCP server. Add a shell build script that compiles `target/release/syslog`, stages a clean bundle directory under `dist/mcpb/syslog-mcp`, copies the binary into `server/syslog`, validates the manifest with `@anthropic-ai/mcpb`, and packs `dist/syslog-mcp-<version>-linux.mcpb`. Keep the bundle local/query-only; it must not add REST, HTTP, or deploy behavior.

**Tech Stack:** Rust binary MCP server, MCP stdio transport, MCPB manifest v0.4, `@anthropic-ai/mcpb` CLI, Bash, Just.

---

## File Structure

| Path | Action | Responsibility |
| --- | --- | --- |
| `mcpb/manifest.json` | Create | Source MCPB manifest for the Linux binary bundle. |
| `scripts/build-mcpb.sh` | Create | Reproducible MCPB staging, validation, and pack pipeline. |
| `Justfile` | Modify | Add `build-mcpb` target. |
| `scripts/check-version-sync.sh` | Modify | Include `mcpb/manifest.json` in version parity checks. |
| `docs/mcp/CONNECT.md` | Modify | Document MCPB install/build path beside direct stdio. |
| `docs/mcp/PUBLISH.md` | Modify | Document MCPB release artifact generation. |
| `CHANGELOG.md` | Modify | Add `0.29.0` entry for MCPB packaging. |
| `Cargo.toml`, `Cargo.lock`, `.claude-plugin/plugin.json`, `server.json` | Modify | Minor version bump to `0.29.0`. |

## Tasks

### Task 1: Add MCPB Manifest

**Files:**
- Create: `mcpb/manifest.json`

- [ ] **Step 1: Create the source manifest**

Add this file:

```json
{
  "$schema": "https://raw.githubusercontent.com/anthropics/mcpb/main/schemas/mcpb-manifest-v0.4.schema.json",
  "manifest_version": "0.4",
  "name": "syslog-mcp",
  "display_name": "Syslog MCP",
  "version": "0.29.0",
  "description": "Query local syslog-mcp SQLite logs through a bundled stdio MCP server.",
  "long_description": "Syslog MCP packages the existing syslog mcp stdio entrypoint as a local MCP Bundle. It is query-only: it reads the configured SQLite database and does not start syslog listeners, HTTP servers, Docker Compose, REST, or deploy flows.",
  "author": {
    "name": "jmagar",
    "url": "https://github.com/jmagar"
  },
  "repository": {
    "type": "git",
    "url": "https://github.com/jmagar/syslog-mcp"
  },
  "documentation": "https://github.com/jmagar/syslog-mcp/tree/main/docs/mcp",
  "server": {
    "type": "binary",
    "entry_point": "server/syslog",
    "mcp_config": {
      "command": "${__dirname}/server/syslog",
      "args": [
        "mcp"
      ],
      "env": {
        "SYSLOG_MCP_DB_PATH": "${user_config.data_dir}/syslog.db",
        "SYSLOG_MCP_RETENTION_DAYS": "0",
        "SYSLOG_MCP_MAX_DB_SIZE_MB": "0",
        "SYSLOG_MCP_RECOVERY_DB_SIZE_MB": "0",
        "SYSLOG_MCP_MIN_FREE_DISK_MB": "0",
        "SYSLOG_MCP_RECOVERY_FREE_DISK_MB": "0",
        "RUST_LOG": "warn"
      }
    }
  },
  "tools": [
    {
      "name": "syslog",
      "description": "Search, tail, summarize, and inspect logs from the configured local syslog-mcp SQLite database."
    }
  ],
  "keywords": [
    "syslog",
    "mcp",
    "logging",
    "homelab"
  ],
  "license": "MIT",
  "compatibility": {
    "platforms": [
      "linux"
    ]
  },
  "user_config": {
    "data_dir": {
      "type": "directory",
      "title": "Data directory",
      "description": "Directory containing syslog.db plus WAL/SHM sidecars. The bundled stdio server reads this database as SYSLOG_MCP_DB_PATH=<data_dir>/syslog.db.",
      "required": true,
      "default": "${HOME}/.syslog-mcp/data"
    }
  }
}
```

- [ ] **Step 2: Validate manifest shape**

Run:

```bash
npx --yes @anthropic-ai/mcpb validate mcpb/manifest.json
```

Expected: command exits `0`.

### Task 2: Add Build Script

**Files:**
- Create: `scripts/build-mcpb.sh`

- [ ] **Step 1: Create the pack script**

Add this file:

```bash
#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

NO_BUILD=0
for arg in "$@"; do
  case "${arg}" in
    --no-build) NO_BUILD=1 ;;
    --help|-h)
      echo "Usage: scripts/build-mcpb.sh [--no-build]"
      exit 0
      ;;
    *)
      echo "unknown argument: ${arg}" >&2
      exit 2
      ;;
  esac
done

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
MANIFEST_VERSION="$(python3 -c 'import json; print(json.load(open("mcpb/manifest.json"))["version"])')"
if [ "${VERSION}" != "${MANIFEST_VERSION}" ]; then
  echo "mcpb manifest version ${MANIFEST_VERSION} does not match Cargo.toml ${VERSION}" >&2
  exit 1
fi

if [ "${NO_BUILD}" -eq 0 ]; then
  cargo build --release
fi

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ ! -x "${TARGET_DIR}/release/syslog" ] && [ -x ".cache/cargo/release/syslog" ]; then
  TARGET_DIR=".cache/cargo"
fi
if [ ! -x "${TARGET_DIR}/release/syslog" ]; then
  echo "missing release binary: ${TARGET_DIR}/release/syslog" >&2
  exit 1
fi

STAGE_DIR="dist/mcpb/syslog-mcp"
OUT_FILE="dist/syslog-mcp-${VERSION}-linux.mcpb"
rm -rf "${STAGE_DIR}"
mkdir -p "${STAGE_DIR}/server"

cp mcpb/manifest.json "${STAGE_DIR}/manifest.json"
install -m 755 "${TARGET_DIR}/release/syslog" "${STAGE_DIR}/server/syslog"

npx --yes @anthropic-ai/mcpb validate "${STAGE_DIR}/manifest.json"
rm -f "${OUT_FILE}"
npx --yes @anthropic-ai/mcpb pack "${STAGE_DIR}" "${OUT_FILE}"
npx --yes @anthropic-ai/mcpb info "${OUT_FILE}" >/dev/null

echo "Built ${OUT_FILE}"
```

- [ ] **Step 2: Make the script executable**

Run:

```bash
chmod +x scripts/build-mcpb.sh
```

- [ ] **Step 3: Run the script**

Run:

```bash
scripts/build-mcpb.sh
```

Expected:

```text
Built dist/syslog-mcp-0.29.0-linux.mcpb
```

### Task 3: Wire Project Commands And Version Checks

**Files:**
- Modify: `Justfile`
- Modify: `scripts/check-version-sync.sh`

- [ ] **Step 1: Add Justfile target**

Add this target after `build-plugin`:

```make
build-mcpb:
    bash scripts/build-mcpb.sh
```

- [ ] **Step 2: Include MCPB manifest in version sync**

In `scripts/check-version-sync.sh`, after the `server.json` block, add:

```bash
if [ -f "mcpb/manifest.json" ]; then
  v=$(python3 -c "import json; print(json.load(open('mcpb/manifest.json')).get('version',''))" 2>/dev/null)
  [ -n "$v" ] && versions+=("mcpb/manifest.json=$v") && files_checked+=("mcpb/manifest.json")
fi
```

- [ ] **Step 3: Verify version sync**

Run:

```bash
bash scripts/check-version-sync.sh --require-changelog
```

Expected: exits `0` and includes `mcpb/manifest.json` in the checked file count.

### Task 4: Version Bump And Docs

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `.claude-plugin/plugin.json`
- Modify: `server.json`
- Modify: `CHANGELOG.md`
- Modify: `docs/mcp/CONNECT.md`
- Modify: `docs/mcp/PUBLISH.md`

- [ ] **Step 1: Bump minor version**

Run:

```bash
scripts/bump-version.sh minor
```

Expected: version-bearing files move from `0.28.x` to `0.29.0`.

- [ ] **Step 2: Add changelog entry**

Add this entry near the top of `CHANGELOG.md`:

```markdown
## [0.29.0] - 2026-05-23

- **MCPB packaging**: Add a Linux MCP Bundle manifest and `scripts/build-mcpb.sh`
  so the existing `syslog mcp` stdio server can be packed as
  `dist/syslog-mcp-<version>-linux.mcpb`.
```

- [ ] **Step 3: Document MCPB connection path**

In `docs/mcp/CONNECT.md`, add a section near direct stdio clients:

````markdown
## MCPB bundle

Build a Linux MCP Bundle from the existing stdio server:

```bash
just build-mcpb
```

The generated `dist/syslog-mcp-<version>-linux.mcpb` bundles the release
`syslog` binary and launches it as:

```bash
server/syslog mcp
```

The bundle is query-only. It reads `syslog.db` from the configured data
directory and does not start the syslog listener, HTTP MCP server, REST API,
Docker Compose, or deployment flows.
````

- [ ] **Step 4: Document release artifact**

In `docs/mcp/PUBLISH.md`, add:

````markdown
### MCPB artifact

Run before publishing a release:

```bash
just build-mcpb
npx --yes @anthropic-ai/mcpb verify dist/syslog-mcp-<version>-linux.mcpb || true
```

The unsigned MCPB is a Linux bundle for local stdio clients. Signing is a
separate distribution step once signing keys are available.
````

### Task 5: Verification, Session Note, PR

**Files:**
- Create: `docs/sessions/2026-05-23-mcpb-package.md`

- [ ] **Step 1: Run focused package gates**

Run:

```bash
npx --yes @anthropic-ai/mcpb validate mcpb/manifest.json
scripts/build-mcpb.sh
```

Expected: both pass and `dist/syslog-mcp-0.29.0-linux.mcpb` exists.

- [ ] **Step 2: Run repo gates**

Run:

```bash
cargo fmt --check
cargo test stdio
cargo clippy -- -D warnings
bash scripts/check-version-sync.sh --require-changelog
bash scripts/validate-marketplace.sh
just check
```

Expected: all pass except `just check` may still fail on the pre-existing module-size guard in `src/cli/args.rs` and `src/cli/dispatch_surface.rs`.

- [ ] **Step 3: Create session note**

Create `docs/sessions/2026-05-23-mcpb-package.md` with branch, PR URL, artifact path, validation commands, and any remaining risks.

- [ ] **Step 4: Commit, push, and open PR**

Run:

```bash
git add .
git commit -m "feat: add MCPB package build"
git push -u origin feat/mcpb-package
gh pr create --base main --head feat/mcpb-package --title "feat: add MCPB package build" --body-file /tmp/syslog-mcpb-pr.md
```

Expected: PR opens against `main`.

## Self-Review

- Spec coverage: The plan builds a real MCPB manifest, package command, version parity, docs, release note, and verification path.
- Placeholder scan: No TBD/TODO placeholders remain.
- Type consistency: Manifest uses MCPB v0.4 fields validated against `@anthropic-ai/mcpb`; scripts use repo-local version conventions.
