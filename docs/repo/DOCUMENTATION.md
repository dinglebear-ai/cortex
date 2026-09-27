# Documentation maintenance contract

## Authority and scope

[AGENTS.md](../../AGENTS.md) is the canonical repository instruction file. In every directory with agent instructions, `AGENTS.md` must be a regular file and both `CLAUDE.md` and `GEMINI.md` must be relative symlinks whose literal target is `AGENTS.md`. Scoped instructions supplement the root; they are not independent copies of it.

The validator checks Git-tracked and non-ignored untracked instruction files, including newly added directories. It deliberately ignores generated/dependency trees ignored by Git. Run:

```bash
bash scripts/check-agent-memory-symlinks.sh
bash scripts/test-agent-memory-symlinks.sh
```

The regression suite covers reversed ownership, missing/broken/wrong links, independent copies, scoped directories, and ignored generated content. CI runs both checks.

## Machine-local instructions

Shared instructions must not contain a developer's hostname, checkout location, private deployment addresses, or personal tool settings. Keep those in a Git-ignored regular file named `AGENTS.override.md`. The ignored relative symlink `CLAUDE.local.md -> AGENTS.override.md` makes it the single local source of truth. Do not use the legacy spelling `CLAUDE.md.local`; preserve old notes outside automatic instruction discovery when migrating.

Codex discovers at most one instruction file per directory and prefers `AGENTS.override.md` over `AGENTS.md`. Therefore begin the local override with an explicit requirement to read and follow the sibling `AGENTS.md` before any work. This is an instruction to the agent, not a claim that Codex automatically imports both files. Claude Code loads `CLAUDE.local.md` alongside `CLAUDE.md`. A clean clone needs neither private file, and ignored local files do not automatically appear in other worktrees.

Filename and precedence references, reviewed 2026-09-27: [Codex custom instructions](https://developers.openai.com/codex/guides/agents-md) and [Claude Code memory](https://code.claude.com/docs/en/memory).

The validator rejects publishable local instruction files, malformed local aliases, and overrides missing a shared-instruction reference. Its fixtures include accidentally force-staged private overrides. Keep the private files ignored even when a task authorizes committing all pre-existing dirty changes.

## Change ownership

| Behavior | Implementation authority | Documentation to review |
| --- | --- | --- |
| Rust/MSRV/SDK/build | `Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml` | `AGENTS.md`, `docs/RUST.md`, setup/prerequisites |
| MCP action/scopes/flags | `src/mcp/actions.rs`, action flags and handler tests | `docs/mcp/TOOLS.md`, `SCHEMA.md`, `docs/INVENTORY.md`, registry coverage tests |
| CLI/REST routes | `src/surfaces/`, `src/cli/`, `src/api/` | `docs/CLI.md`, `docs/api.md`, live surface contracts |
| Ingest/provider coverage | `src/scanner/providers.rs`, provider adapters, `src/agent/` | `docs/ADDING_SOURCES.md`, setup and agent protocol contracts |
| Auth and configuration | `src/config*`, auth policy/OTLP code, env overlay | `docs/CONFIG.md`, `OAUTH.md`, `SECURITY.md`, `.env.example` |
| SQLite/retention | `src/db/`, migrations, maintenance tests | storage/retention contracts, architecture, backup runbooks |
| Plugin skills | `plugins/cortex/`, setup code, validation scripts | plugin docs and runtime assessment include paths |
| Version and release | `release/components.toml`, release-please config and workflows | `RELEASING.md`, `docs/RELEASE.md`, MCP publish/CI docs |
| CI/hooks | `.github/workflows/`, `lefthook.yml`, `xtask/src/pre_push.rs` | contributor guide and testing/release docs |

Avoid copying long inventories into every guide. Link to a canonical reference or registry and retain tests for any intentionally repeated count. Command examples must name existing commands, correct paths, explicit prerequisites, and the appropriate read-only or mutating behavior.

## Current versus historical documentation

`docs/README.md` is the navigation entrypoint. Current guides describe implemented source. Contracts/specs must state whether they are implemented or proposed; the presence of a design document does not prove delivery. Dated plans, research, reports, reviews, and session notes preserve the state at their stated date. Link to them as history, not as present operational instructions. Runbooks are operational documents unless explicitly marked historical.

When reviewing a change, check local Markdown links, source paths, workflow names, license claims, version policy, default auth/storage behavior, and examples against code. Leave third-party/per-file license notices intact; the repository's own license is defined by `LICENSE` and Cargo metadata. Do not replace historical incident evidence with reconstructed current behavior.

## Completion evidence

Documentation work is complete when the relevant source has been inspected, affected current guides agree, aliases validate, applicable doc/registry tests pass, and the final diff has been reviewed. Report a failed or unrun gate explicitly. A local test result, remote CI result, and live deployment verification are separate claims.
