---
title: "Agent Observatory research ledger"
created: 2026-07-31
updated: 2026-08-05
---

# Agent Observatory research ledger

Status: complete planning research
Verified: 2026-07-31
Cortex revision: `bd3c246696312e87fc76720557b7d0dfaae46441`
Aurora revision: `19e87c6b26dd32eb3be3db4d8b6fb32c99cd1d6d`

## Purpose

This ledger records the evidence behind the Cortex Agent Observatory design. The specification states what Cortex must do; this document records why those requirements exist and which upstream contracts were current on July 31, 2026.

The product is a repository-centered live view of AI agent work. An operator starts at a repository, expands worktrees and branches, sees active and recent sessions, then inspects one session as a unified timeline containing transcript messages, commands, shell history, exact Git state and commits, MCP calls, hooks, skills, LLM invocations, OTLP logs, traces, metrics, and host health.

## Revisions reviewed

### Cortex

- Repository: `dinglebear-ai/cortex`
- Revision: `bd3c246696312e87fc76720557b7d0dfaae46441`
- Version: `3.11.1`
- Rust edition/MSRV: 2024 / 1.97.1
- SQLite schema: 43
- Planning worktree: `/home/jmagar/workspace/cortex/.worktrees/agent-observatory-plan-20260731`
- Branch: `docs/agent-observatory-plan-20260731`

### Aurora

- Repository: `dinglebear-ai/aurora`
- Revision: `19e87c6b26dd32eb3be3db4d8b6fb32c99cd1d6d`
- Package version: `0.5.1`
- Registry inventory: 176 items, including 79 UI primitives, 73 composed blocks, 9 pages, 3 styles, 2 themes, and one base bundle

The mutable discovery registry is `https://aurora.tootie.tv/r/{name}.json`. Production installation must use the immutable raw GitHub URL pinned to the full reviewed commit, per Aurora's `docs/versioning.md`.

## Current Cortex capability map

### Already present

- Near-live Claude, Codex, and Gemini transcript ingestion with checkpoints and session metadata.
- `/api/sessions` inventory, full-text search, correlation, context, incident, MCP, hook, skill, and LLM invocation routes.
- Claude command capture through `CLAUDE_CODE_SHELL_PREFIX`, including cwd, duration, exit, PID, host, and native session ID.
- Atuin and shell-history ingest with cwd/session evidence.
- Graph links among sessions, projects, hosts, commands, MCP activity, hooks, skills, and inferred commit activity.
- Project inventory rooted at `~/workspace`, including branch, short HEAD, dirty state, ahead/behind, and worktree paths.
- OTLP/HTTP protobuf logs with `session.id`, project path, service/host fields, and trace/span IDs.
- A static investigation shell with Cytoscape, evidence, and timeline panels.

### Material gaps

- `AiSessionEntry` is an aggregate, not a durable lifecycle record.
- No first-class run status, parent/subagent relation, worktree history, exact commit attribution, or durable ordered event stream.
- Project inventory is a snapshot and does not persist detailed state for every worktree.
- Current Git commit graph entities are inferred from command text and do not carry exact SHAs.
- OTLP `/v1/traces` and `/v1/metrics` intentionally return 404.
- Remote shell-history project attribution is weaker than local Atuin attribution.
- The current three-file web app is not Next.js and does not consume Aurora registry source.
- The current UI refreshes on demand rather than receiving authenticated live updates.
- `CORTEX_AGENT_AI_TRANSCRIPTS` is operationally misleading: it controls only remote transcript forwarding inside `cortex heartbeat agent`, not the independent local `cortex sessions watch` ingestion path.

### Environment-name correction

Decision: rename the forwarding switch to `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`, matching `CORTEX_AGENT_COMMAND_FORWARD` and `CORTEX_AGENT_SHELL_HISTORY_FORWARD`. The legacy `CORTEX_AGENT_AI_TRANSCRIPTS` name remains a deprecated compatibility alias throughout Cortex 3.x. New/generated configuration uses only the replacement. The replacement is authoritative when both are set; conflicts emit an explicit warning. Doctor may rewrite only unambiguous legacy configuration under `--fix --yes`. The compatibility alias is scheduled for removal in Cortex 4.0.

This rename changes no behavior: it does not enable or disable the local sessions watcher, local transcript parsing, or direct SQLite ingestion.

## July 2026 upstream contracts

### Next.js and React

- July 20, 2026 Next.js security release: 16.2.11 is the Active LTS security floor. Source: https://nextjs.org/blog
- Static App Router export uses `output: 'export'` and emits assets any static server can host. Source: https://nextjs.org/docs/app/guides/static-exports
- Static export excludes request-dependent route handlers, cookies, rewrites, headers, proxy, server actions, and other Node-runtime features. Live data must call Cortex from client components.
- Static routes retain route-level code splitting and client navigation. Source: https://nextjs.org/docs/app/guides/single-page-applications
- Official test guidance supports Playwright and Vitest. Sources: https://nextjs.org/docs/app/guides/testing/playwright and https://nextjs.org/docs/app/guides/testing/vitest

Aurora already pins Next.js 16.2.11, React 19.2.7, React DOM 19.2.7, Tailwind 4.3.3, TypeScript 5.9.3, shadcn 4.13.1, and pnpm 10.33.2. Cortex should align with those exact versions for the initial implementation.

### Browser streaming and authentication

- Native `EventSource` accepts a URL and only a `withCredentials` option; it cannot set Cortex's bearer header. Source: https://developer.mozilla.org/en-US/docs/Web/API/EventSource/EventSource
- Fetch Requests support caller-provided headers. Source: https://developer.mozilla.org/en-US/docs/Web/API/Request/headers
- A fetch response body is a `ReadableStream` that can be consumed incrementally. Sources: https://developer.mozilla.org/en-US/docs/Web/API/Response/body and https://developer.mozilla.org/en-US/docs/Web/API/ReadableStream/getReader

Decision: expose standard `text/event-stream`, but connect through authenticated `fetch()` plus `eventsource-parser`. Tokens never appear in query strings, logs, local storage, or static HTML.

### OpenTelemetry

- Semantic Conventions 1.43.0 was current. Source: https://opentelemetry.io/docs/specs/semconv/
- Session conventions are Development and define a session as logs, events, and spans sharing `session.id`. Source: https://opentelemetry.io/docs/specs/semconv/general/session/
- `session.start` and `session.end` are recognized lifecycle events, but their Development status means Cortex needs its own versioned run identity and fallback lifecycle rules.

Decision: flatten query-critical fields while preserving bounded resource, scope, attributes, span events, links, exemplars, and metric values as JSON for future semantic-convention upgrades.

### Agent providers

#### Claude Code

Current official monitoring docs state that Claude Code exports metrics, logs/events, and optional traces through OTLP; shared fields include `session.id`; tracing links prompts, API calls, tools, and hooks. Hook inputs include `session_id`, `transcript_path`, and `cwd`, with subagent fields on relevant hooks.

Sources:

- https://code.claude.com/docs/en/monitoring-usage
- https://code.claude.com/docs/en/hooks

Decision: hooks are high-confidence lifecycle/worktree evidence; trace and event content must respect Anthropic's opt-in content gates and Cortex's own scrubbing/caps.

#### Codex

Codex's generated config schema supports independent OTLP log, metric, and trace exporters, HTTP or gRPC transport, headers, TLS, and static span attributes.

Sources:

- https://github.com/openai/codex/blob/main/codex-rs/core/config.schema.json
- https://github.com/openai/codex/blob/main/codex-rs/core/src/config/schema.md

Decision: use the generated schema as the compatibility contract. Report freshness per signal because coverage may differ by Codex entry point.

#### Gemini CLI

Gemini's official telemetry docs describe logs, metrics, and traces, common `session.id`, `gen_ai.conversation.id`, tool/file/API events, token metrics, agent-run metrics, worktree-active metadata, and optional detailed trace content.

Source: https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/telemetry.md

Decision: normalize both session identifiers as evidence, preserve original attributes, and honor prompt/detail privacy configuration.

### Git worktrees

`git worktree list --porcelain` is the stable scripted format and `-z` provides NUL termination. Records can expose path, HEAD, branch/detached, bare, locked, prunable, and reason fields.

Source: https://git-scm.com/docs/git-worktree

Decision: parse `git worktree list --porcelain -z`; never scrape human-formatted output.

## Aurora consumption contract

Every visible primitive or product block must come from the pinned Aurora registry. Feature code may provide layout and data adapters, but it must not create a parallel button, badge, table, dialog, tabs, input, sidebar, timeline, terminal, status, or empty-state component.

Install an explicit rendered set plus its complete transitive registry graph, not all 176 items. Record top-level items and the Aurora SHA in `web/aurora.lock.json`. Initial top-level items:

- `aurora-base`, `aurora-sidebar-block`, `aurora-resizable-panels`
- `aurora-data-table`, `aurora-filter-bar`, `aurora-timeline`, `aurora-chart`
- `aurora-status-indicator`, `aurora-log-viewer`, `aurora-terminal-block`
- `aurora-tool-calls`, `aurora-code-workspace`
- `aurora-ai-conversation`, `aurora-ai-message`, `aurora-ai-agent`
- `aurora-ai-plan`, `aurora-ai-task`, `aurora-ai-tool`
- `aurora-ai-commit`, `aurora-ai-branch`
- Aurora empty, loading, navigation, overlay, feedback, and command-palette primitives used by the final screen inventory

Aurora fonts must be vendored from the pinned revision into Cortex's export. Production must not depend on Google Fonts or the mutable Aurora host at runtime.

## Dependency decision

### Rust runtime

No new Rust runtime crate is required.

| Need | Existing Cortex capability |
| --- | --- |
| Git/control-path watches | `notify` |
| projector, broadcast, cancellation | `tokio` |
| SSE | `axum::response::sse` |
| stream adapters | `futures-util` |
| OTLP protobuf | `opentelemetry-proto`, `prost` |
| persistence | `rusqlite`, `r2d2_sqlite` |
| hashing and keys | `sha2`, existing length-prefixed key patterns |
| JSON | `serde`, `serde_json` |
| Git commands | existing bounded inventory process runner and system Git |

A standard-library `build.rs` can generate an embedded asset table. Adding `rust-embed`, `include_dir`, `mime_guess`, `tokio-stream`, `async-stream`, UUID, or ULID would add supply-chain surface without closing a gap.

### Frontend runtime

Pin through `pnpm-lock.yaml`:

| Package | Initial version | Purpose |
| --- | ---: | --- |
| `next` | 16.2.11 | security floor and Aurora alignment |
| `react`, `react-dom` | 19.2.7 | Aurora alignment |
| `eventsource-parser` | 3.1.0 | authenticated streamed SSE parsing |
| `@tanstack/react-virtual` | 3.14.8 | bounded DOM for long append-follow timelines |
| Aurora transitive dependencies | pinned registry graph | all visible UI |

Do not add a general state store, query framework, date library, markdown renderer, or new graph library in the first release.

### Frontend build/test

- TypeScript 5.9.3, Tailwind 4.3.3, shadcn 4.13.1
- Playwright 1.61.1 and axe-core Playwright 4.12.1
- Vitest plus Testing Library at reviewed compatible versions
- `parse5` 8.0.1 for deterministic exported-HTML parsing and CSP hash generation

## Frozen architecture decisions

1. Keep Cortex a single production Rust binary.
2. Export the Next App Router statically under `/app`; no production Node server or BFF.
3. Use a restart-safe source projector so existing ingestion paths remain decoupled from the observatory.
4. Use authenticated fetch streaming with durable replay and reset behavior.
5. Observe Git control paths and reconcile with bounded porcelain commands.
6. Attribute exact commits from HEAD transitions and Git object metadata, not command text.
7. Implement OTLP logs, traces, and metrics; preserve raw bounded attributes.
8. Generate route-specific CSP script hashes from static output. Never enable `unsafe-inline` for scripts.
9. Permit only evidence-backed run/worktree links, each with confidence and trust level.
10. Apply retention, storage-budget, backup, integrity, and privacy rules to every new table.

## Principal risks and required proof

| Risk | Required mitigation |
| --- | --- |
| inotify exhaustion | watch Git control paths, periodic reconciliation, large-tree test |
| duplicate replay | unique event keys plus transactional cursors, replay-twice test |
| ambiguous attribution | multiple evidence links with trust/confidence, concurrency fixtures |
| rebases and detached HEAD | preserve observations, reachability flag, force-move tests |
| unbounded data | response caps, JSON caps, pagination, virtualization, retention tests |
| sensitive content | source gates, scrubbing, identity hashing, response-redaction tests |
| slow/stale stream client | bounded broadcast, durable outbox, cursor expiry reset tests |
| CSP regression | build-time hashes and zero browser CSP violations |
| Aurora drift | full SHA pin, lock manifest, import and generated-file audit |
| signal gaps | per-signal freshness, never infer inactivity from a missing provider signal |

## Non-goals for the first production release

- Remote session control or command execution
- Browser source editing
- Full source diff retention by default
- A general Git hosting UI
- A separate Node production service
- Replacing the canonical log table
- Installing all Aurora items without rendering them
- Treating inferred relationships as verified facts

## Production evidence required

Completion requires fresh-database and schema-43 upgrade tests, idempotent backfill and crash-resume tests, temporary Git/worktree integration tests, Claude/Codex/Gemini OTLP fixtures, authenticated stream replay/reset/load tests, desktop/mobile browser tests, keyboard and axe gates, zero CSP violations, real Rust-binary asset serving, retention/backup/integrity/restore verification, documented privacy defaults, and a clean full pre-push plus release build.
