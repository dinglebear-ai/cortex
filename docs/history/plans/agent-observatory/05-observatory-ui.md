# Phase 5: full Agent Observatory Next.js application

Prerequisite: Phase 4 static shell is served by real Cortex and Phase 3 APIs are stable.
Phase gate: G6 Web.

## AO-079 Implement in-memory authentication context

- **Deliverable:** token submit/clear/unauthorized state with no persistence.
- **Files:** `web/lib/auth-context.tsx`, Aurora connection card, tests.
- **RED:** test finds token in localStorage/cookie/URL or fetch proceeds before token.
- **GREEN:** React context stores token only in state; unauthorized clears and aborts clients.
- **Proof:** unit and Playwright reload/clear tests prove token disappears and storage remains empty.
- **Gate:** no token logging or DOM value after submit.
- **References:** UI authentication state and spec §12.3.

## AO-080 Implement typed API client

- **Deliverable:** bearer-authenticated fetch wrappers for all read endpoints with typed errors, abort, cap, and query serialization.
- **Files:** `web/lib/cortex-api.ts`, tests.
- **RED:** URL encoding, repeated filters, 401, 404, malformed JSON, cancellation fixtures fail.
- **GREEN:** explicit token argument and shared response validator/narrow runtime guards.
- **Proof:** mocked requests exactly match OpenAPI paths/headers; no Authorization value in error text.
- **Gate:** no global mutable token singleton.
- **References:** OpenAPI and TypeScript contract.

## AO-081 Implement authenticated SSE fetch client

- **Deliverable:** stream client using fetch, `eventsource-parser`, AbortController, Last-Event-ID, retry/backoff, and typed envelopes.
- **Files:** `web/lib/agent-stream.ts`, tests.
- **RED:** arbitrary chunk boundaries, multiline data, comments, duplicate IDs, malformed frame, abort, 401 fixtures fail.
- **GREEN:** incremental parser and reconnect state callbacks.
- **Proof:** chunk-fuzz test reconstructs exact event sequence; Authorization is header only.
- **Gate:** no native EventSource and no URL token.
- **References:** research browser streaming sources.

## AO-082 Implement observatory reducer

- **Deliverable:** snapshot + stream reducer updates repositories, worktrees, runs, counts, cursor, and reset state idempotently.
- **Files:** `web/lib/observatory-reducer.ts`, tests.
- **RED:** replay/live duplicate, out-of-order older ID, deletion/removal, reset fixtures corrupt state.
- **GREEN:** normalized maps/order arrays with monotonic cursor and explicit reset.
- **Proof:** applying any event sequence twice yields same final state; reset preserves URL selection only.
- **Gate:** reducer is pure and time injected.
- **References:** UI live state machine.

## AO-083 Implement URL selection and filter codec

- **Deliverable:** route search-param parser/serializer for repository/worktree/run/tab/event filters.
- **Files:** `web/lib/observatory-route.ts`, tests.
- **RED:** Unicode run keys, repeated kind/status, invalid IDs/tabs, back/forward fixtures fail.
- **GREEN:** canonical serialization omitting defaults and secrets.
- **Proof:** parse/serialize round-trip and no token key accepted.
- **Gate:** filters remain server-truth, not client partial search.
- **References:** UI route map.

## AO-084 Build global Aurora application shell

- **Deliverable:** launcher, sidebar navigation, connection status, top-level route landmarks, command-palette hook.
- **Files:** layout/page/components and tests.
- **RED:** route lacks expected headings/nav/focus or uses bespoke primitives.
- **GREEN:** compose Aurora sidebar, breadcrumb, tooltip, banner, toast, command palette.
- **Proof:** component audit and Testing Library landmark/focus tests pass.
- **Gate:** no observatory data layout yet.
- **References:** UI Application shell.

## AO-085 Build responsive repository/worktree navigator

- **Deliverable:** host/repository/worktree tree with status, dirty/ahead/behind, active counts, removed filter, desktop panel/mobile sheet.
- **Files:** `components/observatory/repository-navigator*`, tests.
- **RED:** keyboard expand/select, mobile sheet, empty/loading/error, removed rows fail.
- **GREEN:** compose Aurora sidebar/tree/scroll/status/sheet/skeleton/empty elements.
- **Proof:** user-event tests cover arrows/Enter/Escape/focus restore; snapshot fixture renders exact state labels.
- **Gate:** inferred mapping visibly labeled.
- **References:** UI repository navigator.

## AO-086 Build virtualized run list and filter bar

- **Deliverable:** sortable/filterable run table/list with mobile variant and server pagination.
- **Files:** run-list/filter components, hooks/tests.
- **RED:** 5,000-run fixture mounts thousands of rows, URL filters wrong, pagination duplicates.
- **GREEN:** Aurora data table/filter bar/status plus virtualization threshold.
- **Proof:** DOM row count stays bounded; server request fixture matches filters/cursor.
- **Gate:** status has text/icon, not color alone.
- **References:** UI Run list.

## AO-087 Build selected-run header and ambiguity evidence

- **Deliverable:** run identity, status, branch/HEAD, freshness, trust, copy action, ambiguous worktree sheet.
- **Files:** run-header/evidence components/tests.
- **RED:** weak relation silently shown as primary or copy includes token/URL secret.
- **GREEN:** Aurora agent/branch/commit/status/breadcrumb/sheet composition.
- **Proof:** verified and ambiguous fixtures render distinct accessible labels and evidence ordering.
- **Gate:** exact timestamps accessible through tooltip/text.
- **References:** UI selected run header.

## AO-088 Build virtualized unified timeline shell

- **Deliverable:** paginated ascending/descending timeline, prepend without scroll jump, append-follow, Return to live.
- **Files:** timeline/hooks/tests; `@tanstack/react-virtual`.
- **RED:** 10,000 events mount unbounded DOM; prepend shifts selected row; append steals scroll.
- **GREEN:** Aurora timeline semantics plus virtualizer and anchor restoration.
- **Proof:** unit/browser tests assert <=250 event rows, stable scroll anchor, live-follow behavior.
- **Gate:** keyboard selected item remains stable after updates.
- **References:** UI Timeline and performance contract.

## AO-089 Add transcript event renderers

- **Deliverable:** user/assistant/system/provider plan/task messages grouped by actor with safe text/code and metadata toggle.
- **Files:** event renderer registry, transcript components/tests.
- **RED:** unsafe HTML fixture executes/renders markup, actor grouping/toggle/search mismatch.
- **GREEN:** Aurora AI conversation/message/plan/task/tool components; text-first rendering.
- **Proof:** XSS fixtures remain inert; match navigation constrained to selected run.
- **Gate:** content hidden when privacy response omits payload.
- **References:** UI Transcript.

## AO-090 Add command and shell-history renderers

- **Deliverable:** terminal timeline item plus commands table/detail sheet and failed/long/Git filters.
- **Files:** command components/tests.
- **RED:** secret marker, cwd, duration, exit, source, and failed state lost.
- **GREEN:** Aurora terminal/data-table/filter/sheet/badge components.
- **Proof:** fixtures render scrubbed content and exact metadata; no raw secret in DOM.
- **Gate:** command expansion lazy-loads payload when required.
- **References:** UI Commands.

## AO-091 Add Git state, branch, and commit views

- **Deliverable:** worktree card, HEAD transition timeline, exact commit list, changed paths, confidence evidence.
- **Files:** Git tab/components/tests.
- **RED:** inferred command commit looks exact; reset/rebase history disappears.
- **GREEN:** Aurora branch/commit/timeline/data-table/code-workspace/alert composition.
- **Proof:** exact/inferred/reachable/unreachable fixtures have distinct labels and retained history.
- **Gate:** no full diff request or display.
- **References:** UI Git and spec §7.

## AO-092 Add MCP, hook, skill, and LLM views

- **Deliverable:** combined faceted workspace and timeline renderers.
- **Files:** activity tab/renderers/tests.
- **RED:** tool input/output, hook duration/exit, skill and LLM rows cannot be distinguished or filtered.
- **GREEN:** Aurora tool-calls/AI-tool/timeline/table/tabs/alert.
- **Proof:** one fixture per source supports filter, expand, and source evidence.
- **Gate:** payload caps/errors shown safely.
- **References:** UI MCP/hooks/skills.

## AO-093 Add telemetry span and metric views

- **Deliverable:** trace tree/waterfall detail, metric summary/cards/charts/table fallback, freshness.
- **Files:** dynamically imported telemetry components/tests.
- **RED:** parent/child ordering, error status, long span, histogram/summary fallback, no-signal state fail.
- **GREEN:** Aurora chart/card/table/status/resizable detail; feature-specific span layout with Aurora tokens.
- **Proof:** deterministic trace fixture renders correct nesting/timing and accessible text table; metrics fixture renders values/units.
- **Gate:** no new chart dependency and module is lazy-loaded.
- **References:** UI Telemetry.

## AO-094 Add evidence and repository overview states

- **Deliverable:** raw bounded provenance table/detail and repository overview when no run selected.
- **Files:** evidence/repository-overview components/tests.
- **RED:** trust/confidence/source/interval absent; no-selection shows fake data.
- **GREEN:** Aurora data table/code/badge/chart/status/commit/branch/empty components.
- **Proof:** empty/partial/stale/error fixtures render correct messages and no invented evidence.
- **Gate:** raw metadata remains redacted/bounded.
- **References:** UI Evidence and Repository overview.

## AO-095 Integrate snapshot, stream, reset, and reconnect workflow

- **Deliverable:** route controller fetches snapshots, connects at cursor, applies updates, refetches targeted entities, handles reset/reconnect/unauthorized.
- **Files:** agents route/controller/hooks/tests.
- **RED:** controlled server fixture demonstrates duplicate, reset loop, stale selection, retry storm, or token-clear leak.
- **GREEN:** state machine and bounded jittered retries; abort on clear/filter/unmount.
- **Proof:** Playwright test drives live event, disconnect/reconnect, expired reset, unauthorized, and sees each update once.
- **Gate:** one live stream per route instance and no orphan fetches.
- **References:** UI live stream state machine.

## AO-096 Migrate investigation workspace to Next/Aurora and remove legacy app

- **Deliverable:** current Ask/graph/evidence/timeline functionality under `/app/investigate/` with route/API parity; old hand-written files removed only after tests.
- **Files:** Next investigate route/components, dynamic Cytoscape import, web router/tests, delete legacy `web/app/*` when green.
- **RED:** parity test against existing route fixtures fails for version/stats/hosts/tail/Ask/graph/evidence/token behavior.
- **GREEN:** compose Aurora shell/panels/timeline/evidence; reuse typed client and auth context.
- **Proof:** old and new acceptance vectors match, then legacy asset references/files removed; deep link still works.
- **Gate:** rollback remains possible by prior binary/package, not dead duplicate source.
- **References:** current `web/app/app.js`, investigation API contracts.

## Phase 5 gate

```bash
pnpm --dir web lint
pnpm --dir web typecheck
pnpm --dir web test -- --coverage
pnpm --dir web build
node web/scripts/audit-aurora.mjs
pnpm --dir web e2e
cargo test web_app --lib
cargo test --test agent_observatory_web
```

Required browser projects:

- Chromium 1440 desktop
- Chromium Pixel-class mobile
- reduced motion
- 200% zoom workflow

Required proof:

- critical repository -> worktree -> run -> timeline workflow keyboard-only
- no serious/critical axe findings
- no CSP console violation
- token absent from storage, URL, logs, error text, and DOM after connect
- reconnect/reset/unauthorized flows deterministic
- 10,000-event timeline bounded and responsive
- every rendered primitive/block passes Aurora audit
- migrated investigation route has acceptance parity before legacy removal
