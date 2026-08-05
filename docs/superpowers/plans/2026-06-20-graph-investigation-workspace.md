# Graph Investigation Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement bead `syslog-mcp-6b9tk`: a live embedded Cortex investigation workspace with authenticated `/api/v1` routes, a safe Ask + Explain vertical slice, pressure-first BAM Mode, bounded graph/evidence rendering, and deterministic API/browser verification.

**Architecture:** Keep Cortex as one Rust service. Add app-facing `/api/v1` routes inside the existing forced-auth REST router, use explicit browser-safe DTOs rather than raw graph/service models, put Ask/BAM orchestration in `src/app/services/`, and serve a Vite-built TypeScript SPA under `/app/investigate` with fallback scoped only to `/app/*`. Implement the feature as vertical slices: first a safe Ask loop that actually works, then BAM, then graph/evidence expansion.

**Tech Stack:** Rust 1.86 / edition 2024, Axum 0.8, Tokio, SQLite/rusqlite, existing Cortex graph/log/heartbeat services, Vite 8.0.16, TypeScript 6.0.3, Cytoscape 3.34.0, Vitest 4.1.9, Playwright 1.61.0.

## Global Constraints

- Existing `/api/*` routes remain compatible.
- `/v1/logs` remains the OTLP-compatible ingest endpoint and must not become the general app API namespace.
- New app-facing routes live under `/api/v1`.
- `/api/v1` must be mounted under the same forced bearer/OAuth auth layer as existing `/api/*`.
- Do not inject `CORTEX_API_TOKEN` or any static secret into HTML or JavaScript.
- Browser auth is memory-only bearer entry for v1: no URL token, no localStorage, no sessionStorage, no token echo in errors, `autocomplete="off"`, and a visible clear-token action.
- Ask + Explain and BAM Mode use thin server-side orchestration in v1, not browser fanout.
- Orchestrators live in `src/app/services/`; Axum handlers stay thin adapters.
- Every investigation response includes shared metadata for `server_version`, `schema_version`, graph projection status when known, degraded reasons, truncation/partial flags, auth state, source watermark when known, payload limits, and version-skew information when known.
- Budget metadata must report measured/enforced work, not intent. Do not return fake `db_ops = 1` style counters.
- Every investigation request has pre-execution gates for max DB operations, graph calls, graph nodes/edges, evidence rows, log rows, timeline buckets, candidate explanations, wall time, payload bytes, and POST body size.
- Budget breaches return partial responses with visible metadata, not hidden failures.
- Browser-facing log/evidence/timeline context is safe summary data only.
- Do not return raw `LogEntry`, raw frames, raw metadata, raw artifact bodies, `metadata_json`, `ai_transcript_path`, `cache_path`, `source_path`, `source_id`, `source_signature_hash`, `metadata_path`, or secret-like values to the app.
- `/api/v1/graph/*` routes must return explicit `AppGraph*` browser DTOs, not raw graph response models wrapped in an envelope.
- Claims carry explicit `claim_type`: `verified`, `supported_correlation`, `weak_correlation`, or `open_question`.
- Timing alone must never be promoted to causal language.
- Retrieved logs, graph labels, evidence text, transcript text, and user prompts are passive untrusted data. They cannot alter `claim_type`, create tool/action names, create URLs to fetch, or become causal assertions.
- Static SPA fallback is scoped only under `/app/*` and must not intercept `/mcp`, `/api/*`, `/api/v1/*`, `/health`, `/v1/logs`, or other operational routes.
- Use a proven browser graph visualization library instead of hand-rolling graph physics.
- Bundle and pin frontend dependencies; do not load graph libraries from a CDN.
- Serve app HTML with CSP and `Cache-Control: no-store`; fingerprinted JS/CSS assets may use immutable caching.
- Use Aurora tokens and a dense operator-tool layout.
- Saved investigation cases, exports, collaboration, persistent caches, same-origin app sessions, advanced scoring, LLM narrative generation, and production-data E2E are deferred.
- Execute from a fresh worktree off updated `main` after PR #90 is merged or rebased, not from `codex/fix-cortex-review-findings`.
- Use beads for tracking: claim each child bead before its task and close it only after its validation passes.
- Because this is a feature branch, run `cargo xtask bump-version minor` before the final PR unless the repository maintainer explicitly chooses a different bump.

## Engineering Review Findings Applied

- The plan is now vertical-slice-first: Ask workspace first, BAM second, graph/evidence expansion third.
- Raw graph wrappers are forbidden. `/api/v1` graph routes must convert existing graph models into browser DTOs.
- Budget accounting must be real: pre-execution gates, timeout, measured counters, serialized payload measurement, and partial metadata.
- Ask cannot pass raw natural-language prompts directly to FTS5 as the main search path. It must sanitize/fallback and tolerate FTS errors.
- BAM must use heartbeat/fleet/correlate-state and rollup-backed baseline comparison before service/container fanout.
- Static serving uses Vite `base`, fingerprinted asset routes, Docker build/copy steps, and full composed-router tests.
- `/api/v1` errors as well as successes get `Cache-Control: no-store`.
- CSP is required for the app shell.
- Browser token UX must be explicit and non-persistent.
- Evidence hydration must be lazy or batched; no per-edge fanout on first render.
- Verification must include SQLite starvation/concurrency and browser graph render-budget tests.

## File Structure

- Create `src/app/models/investigation.rs`: browser-safe DTOs, envelope metadata, budgets, claims, safe graph summaries, safe evidence/log/timeline summaries, Ask responses, and BAM responses.
- Modify `src/app/models.rs`: export `investigation::*`.
- Create `src/app/services/investigation.rs`: request-local budget gates, safe conversion helpers, request-local graph cache, Ask orchestration, BAM orchestration.
- Modify `src/app/services.rs`: register the new service module and test sidecar.
- Create `src/api_v1.rs`: thin Axum handlers for `/api/v1/investigations/ask`, `/api/v1/investigations/bam`, and explicit safe `/api/v1/graph/*` routes.
- Modify `src/api.rs`: mount `api_v1::router()` inside the existing forced-auth route set before the auth layer is applied; do not mount it through static routes.
- Modify `src/main.rs`: expose a full composed-router test helper if one does not already exist, so app/API/MCP/health/OTLP fallback behavior is tested at production composition level.
- Modify `src/lib.rs`: expose `api_v1` and the static app module.
- Create `src/api_v1_tests.rs`: route, auth, no-store, CSP, forbidden-field, request-size, and composed-router fallback tests.
- Create `src/app/services/investigation_tests.rs`: service-level budget, ambiguity, claim-type, prompt-injection, pressure-first, rollup baseline, graph cache, and redaction tests.
- Create `web/investigate/package.json` and `web/investigate/package-lock.json`: pinned frontend dependencies and scripts.
- Create `web/investigate/index.html`: Vite entry shell with no embedded secret.
- Create `web/investigate/src/main.ts`: app bootstrap.
- Create `web/investigate/src/api.ts`: memory-only bearer API client and typed fetch helpers.
- Create `web/investigate/src/types.ts`: narrow UI types for only fields the UI reads.
- Create `web/investigate/src/state.ts`: case state, request cancellation, graph render debouncing.
- Create `web/investigate/src/render.ts`: safe text-node rendering for app regions.
- Create `web/investigate/src/graph.ts`: Cytoscape renderer with caps, hidden counts, render budget, stale-layout cancellation, and no per-edge evidence fanout.
- Create `web/investigate/src/styles.css`: Aurora-token dense operator layout.
- Create `web/investigate/src/render.test.ts`: Vitest tests for safe rendering, auth state, degraded states, and retained-out evidence.
- Create `web/investigate/playwright.config.ts` and `web/investigate/tests/workspace.spec.ts`: browser tests for app load, auth, Ask, BAM, degraded states, graph nonblank, and XSS fixtures.
- Modify `Cargo.toml`: add `tower-http` `fs` feature only if the implementation uses `ServeDir`/`ServeFile`.
- Modify `config/Dockerfile`: build or copy frontend assets in the Docker build context and runtime image.
- Modify `Justfile`: add `investigate-web-install`, `investigate-web-build`, `investigate-web-test`, `investigate-e2e`, and a full composed-router smoke target if useful.
- Modify `docs/api.md` and `docs/INVENTORY.md`: document `/api/v1` and `/app/investigate`.
- Modify version-bearing files through `cargo xtask bump-version minor`.

## Task 1: Browser-Safe DTO Contract

**Files:**
- Create: `src/app/models/investigation.rs`
- Modify: `src/app/models.rs`
- Test: `src/app/models_tests.rs`

**Interfaces:**
- Consumes: existing graph/log/timeline model types only inside conversion helpers.
- Produces:
  - `InvestigationEnvelope<T>`
  - `InvestigationMetadata`
  - `InvestigationBudget`
  - `InvestigationBudgetUsed`
  - `InvestigationClaimType`
  - `InvestigationClaim`
  - `AppEntitySummary`
  - `AppRelationshipSummary`
  - `AppEvidenceSummary`
  - `AppGraphResponse`
  - `AppLogSummary`
  - `AskInvestigationRequest`
  - `AskInvestigationResponse`
  - `BamInvestigationRequest`
  - `BamInvestigationResponse`

- [ ] **Step 1: Write failing DTO denylist tests**

Add tests to `src/app/models_tests.rs` that serialize representative DTOs and assert absence of:

```rust
const FORBIDDEN_BROWSER_FIELDS: &[&str] = &[
    "metadata_json",
    "raw_frame",
    "ai_transcript_path",
    "cache_path",
    "source_path",
    "source_id",
    "source_signature_hash",
    "metadata_path",
    "sk-proj-",
    "Bearer ",
    "password=",
    "token=",
];
```

Test cases:
- `AppLogSummary` containing a secret-like message serializes with the secret marker redacted.
- `AppEvidenceSummary` represents retained-out evidence with `missing_source_reason = "source_log_missing_or_retained_out"` and no raw source fields.
- `InvestigationClaimType::SupportedCorrelation` serializes as `"supported_correlation"`.
- `InvestigationEnvelope<AskInvestigationResponse>` keeps degraded/partial metadata outside result arrays.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test app::models::tests::investigation_ -- --nocapture
```

Expected: FAIL with unresolved investigation DTO types.

- [ ] **Step 3: Implement DTOs with explicit safe fields**

Create `src/app/models/investigation.rs` with the interfaces above. Required details:
- Use `#[serde(rename_all = "snake_case")]` on `InvestigationClaimType`.
- Use `#[serde(deny_unknown_fields)]` on request types.
- `InvestigationMetadata` includes `server_version`, `schema_version`, `graph_projection_status: Option<String>`, `source_watermark: Option<String>`, `degraded_reasons`, `truncated`, `truncation_reasons`, `partial`, `partial_reasons`, `auth_state`, `budget`, `budget_used`, `payload_limit_bytes`, and `version_skew`.
- `AppRelationshipSummary` must not include `relationship_key`.
- `AppEvidenceSummary` must not include raw `source_id`, `source_signature_hash`, or `metadata_path`.
- `AppGraphResponse` uses `AppEntitySummary`, `AppRelationshipSummary`, and `AppEvidenceSummary`; it never embeds raw `GraphRelationship`, `GraphEvidence`, or `GraphEntityCandidate`.

- [ ] **Step 4: Add conversion helpers**

In `src/app/services/investigation.rs` or a focused submodule, add helpers:

```rust
fn app_entity_summary(entity: &GraphEntity) -> AppEntitySummary
fn app_relationship_summary(relationship: &GraphRelationship) -> AppRelationshipSummary
fn app_evidence_summary(evidence: &GraphEvidence) -> AppEvidenceSummary
fn app_log_summary(log: &LogEntry, max_chars: usize) -> AppLogSummary
fn safe_passive_text(input: &str, max_chars: usize) -> String
```

Rules:
- `safe_passive_text` strips control characters except newline/tab, redacts secret-like markers, and truncates.
- `safe_passive_text` must not HTML-escape `<` or `>` in JSON. Browser rendering is responsible for text-node escaping.

- [ ] **Step 5: Export models and run tests**

Modify `src/app/models.rs`:

```rust
mod investigation;
pub use investigation::*;
```

Run:

```bash
cargo test app::models::tests::investigation_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
bd update syslog-mcp-6b9tk.1 --claim
git add src/app/models.rs src/app/models/investigation.rs src/app/models_tests.rs
git commit -m "feat: add investigation browser DTO contract"
```

## Task 2: Authenticated `/api/v1` Ask Vertical Slice

**Files:**
- Create: `src/app/services/investigation.rs`
- Create: `src/app/services/investigation_tests.rs`
- Create: `src/api_v1.rs`
- Create: `src/api_v1_tests.rs`
- Modify: `src/app/services.rs`
- Modify: `src/api.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: DTOs from Task 1; existing `graph_entity_lookup`, `graph_explain`, `search_logs`, `context`, `timeline`, `host_state`, `fleet_state`, `correlate_state`, and `incident_context` primitives where useful.
- Produces:
  - `CortexService::investigation_ask(req: AskInvestigationRequest) -> ServiceResult<InvestigationEnvelope<AskInvestigationResponse>>`
  - `POST /api/v1/investigations/ask`
  - request-local budget gate and graph cache.

- [ ] **Step 1: Write failing service tests**

Create `src/app/services/investigation_tests.rs` with tests for:
- Ambiguous entity returns candidates/open questions and no `verified` claim.
- Prompt-injection text in retrieved logs cannot alter `claim_type`, create tool/action names, create URLs to fetch, or promote timing-only language to causality.
- Hyphenated/plain-English prompts do not crash FTS5; raw prompt search failures become partial metadata and open questions.
- Tiny budget returns partial metadata before running expensive graph explain.
- Request-local graph status/entity resolution cache prevents repeated status/entity lookups inside one Ask request.

Expected failing command:

```bash
cargo test app::services::investigation_tests::ask_ -- --nocapture
```

- [ ] **Step 2: Implement real budget gates**

In `src/app/services/investigation.rs`, implement `InvestigationRunBudget` with:
- `started: Instant`
- `tokio::time::timeout` around service-level async calls made by Ask/BAM
- pre-call checks for DB ops, graph calls, graph nodes/edges, evidence rows, log rows, timeline buckets, candidate explanations, and payload bytes
- measured `budget_used`, not hardcoded counters
- `partial_reasons` for every skipped or timed-out operation
- serialized payload byte measurement before returning the final envelope

Do not attempt to cancel a SQLite job after it starts; instead clamp before selecting expensive primitives and use timeouts around await points.

- [ ] **Step 3: Implement request-local graph cache**

Add a request-local cache struct used by Ask and BAM:

```rust
struct InvestigationGraphCache {
    projection_status: Option<GraphProjectionStatusResponse>,
    entity_by_host: HashMap<String, Option<AppEntitySummary>>,
    explain_by_entity: HashMap<i64, AppGraphResponse>,
}
```

Cache graph projection status, entity resolution, and bounded explain/around results inside one request only.

- [ ] **Step 4: Implement Ask orchestration**

Rules:
- Accept prompt, optional host, optional since/until, and budget.
- Treat prompt parsing as hints only.
- Prefer explicit `host` over text extraction.
- If no explicit host exists, use existing entity resolution/search surfaces to produce candidates/open questions. Do not hardcode a fixed host list.
- Run at most one small `graph_explain` for the initial answer.
- Do not call `/graph/evidence` per edge.
- Use existing graph chain fields as they exist today: `GraphNarrativeChain` has `chain_id`, `summary`, `entities`, `relationships`, `evidence_ids`, and `relationship_ids`.
- Convert all graph/log/evidence output to `App*` DTOs before returning.
- Use `supported_correlation` or `weak_correlation` unless the underlying graph/evidence is explicitly verified and causal wording is not inferred from timing.

- [ ] **Step 5: Add `/api/v1` router under forced auth**

Create `src/api_v1.rs` with a stateless router that is nested by `src/api.rs`. The v1 router must not apply its own alternate auth policy and must not be mounted in the static app router.

Modify `src/api.rs` so the existing forced auth layer wraps both existing `/api/*` and new `/api/v1/*` routes. The order must be:
1. build existing API routes
2. add/nest `/api/v1` routes
3. apply forced `AuthPolicy::Mounted`
4. apply CORS/state

- [ ] **Step 6: Ensure no-store on success and errors**

In `src/api_v1.rs`, implement a response helper that adds:

```text
Cache-Control: no-store
```

to success and error responses. Do not call `crate::api::respond()` directly without adding this header.

- [ ] **Step 7: Add route tests**

`src/api_v1_tests.rs` must prove:
- missing bearer on `/api/v1/investigations/ask` returns `401`, not HTML
- wrong bearer returns `401`, not HTML
- success returns `Cache-Control: no-store`
- error responses return `Cache-Control: no-store`
- unknown query/body fields return `400`
- response JSON contains no denylisted fields or secret-like values
- existing `/api/graph/*` and `/api/*` behavior is unchanged

Run:

```bash
cargo test api_v1_tests app::services::investigation_tests::ask_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/app/services.rs src/app/services/investigation.rs src/app/services/investigation_tests.rs src/api.rs src/api_v1.rs src/api_v1_tests.rs src/lib.rs
git commit -m "feat: add authenticated ask investigation api"
```

Do not close `syslog-mcp-6b9tk.1` until Task 4 adds the safe graph routes.

## Task 3: Embedded App Shell, Static Assets, CSP, And Token UX

**Files:**
- Create: `web/investigate/package.json`
- Create: `web/investigate/package-lock.json`
- Create: `web/investigate/index.html`
- Create: `web/investigate/src/main.ts`
- Create: `web/investigate/src/api.ts`
- Create: `web/investigate/src/render.ts`
- Create: `web/investigate/src/state.ts`
- Create: `web/investigate/src/styles.css`
- Create: `src/static_app.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Modify: `Cargo.toml`
- Modify: `config/Dockerfile`
- Modify: `Justfile`
- Test: `web/investigate/src/render.test.ts`, `src/api_v1_tests.rs`

**Interfaces:**
- Consumes: `/api/v1/investigations/ask`.
- Produces:
  - `/app/investigate`
  - `/app/assets/*` or `/app/investigate/assets/*` for fingerprinted Vite assets
  - memory-only bearer token entry and clear-token action
  - CSP/no-store app shell.

- [ ] **Step 1: Pin frontend package**

Create `web/investigate/package.json` with:

```json
{
  "name": "cortex-investigation-workspace",
  "private": true,
  "version": "1.0.0",
  "type": "module",
  "scripts": {
    "build": "vite build --base=/app/investigate/",
    "test": "vitest run",
    "test:browser": "playwright test"
  },
  "dependencies": {
    "cytoscape": "3.34.0"
  },
  "devDependencies": {
    "@playwright/test": "1.61.0",
    "typescript": "6.0.3",
    "vite": "8.0.16",
    "vitest": "4.1.9"
  }
}
```

Run:

```bash
npm install --package-lock-only --prefix web/investigate
```

- [ ] **Step 2: Add app shell and token UX**

The app shell must:
- render Ask bar, answer stack, graph canvas placeholder, evidence panel placeholder, and timeline/log strip placeholder
- use `input.type = "password"` for token
- set `autocomplete = "off"`
- store the token only in module memory
- provide a clear-token button that blanks memory and the input
- never put token in URL, localStorage, sessionStorage, logs, thrown errors, or visible response text

- [ ] **Step 3: Add safe rendering tests**

`web/investigate/src/render.test.ts` must prove:
- hostile text renders via `textContent`
- auth failure state is visible and does not echo token
- metadata-only degraded/partial/truncated state renders even with empty result arrays
- retained-out evidence displays as "evidence exists, source missing" rather than "no evidence"

Run:

```bash
npm test --prefix web/investigate
```

- [ ] **Step 4: Implement static serving without compile-time dist dependency**

Do not embed the built `dist/index.html` at Rust compile time unless CI/Docker guarantees the frontend build before Rust compile. Preferred v1:
- add `tower-http` `fs` feature
- serve `web/investigate/dist/index.html` and fingerprinted assets with `ServeDir`/`ServeFile`
- set Vite base to `/app/investigate/`
- serve app shell with `Cache-Control: no-store`
- serve fingerprinted assets with immutable caching
- add CSP header:

```text
object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'
```

- [ ] **Step 5: Add Docker build/copy**

Update `config/Dockerfile` so the Docker context includes `web/investigate` and the image contains `web/investigate/dist`. If Docker builds the frontend inside the image, install Node dependencies from `package-lock.json`; if the release context copies prebuilt assets, add a smoke test that fails when `dist/index.html` or fingerprinted assets are stale/missing.

- [ ] **Step 6: Add full composed-router tests**

Do not test fallback isolation only through `api::router(state)`. Add or reuse a full composed-router helper at the same composition level as production `src/main.rs`.

Tests must prove:
- `/app/investigate` returns HTML with CSP and `Cache-Control: no-store`
- `/app/investigate` body does not contain the API token
- fingerprinted assets resolve
- missing/wrong bearer on `/api/v1/*` returns `401`, not HTML, after static app merge
- `/api/*`, `/api/v1/*`, `/mcp`, `/health`, and `/v1/logs` are not intercepted by `/app/*` fallback

- [ ] **Step 7: Run and commit**

Run:

```bash
npm run build --prefix web/investigate
npm test --prefix web/investigate
cargo test api_v1_tests -- --nocapture
```

Expected: PASS.

Commit:

```bash
bd update syslog-mcp-6b9tk.2 --claim
git add Cargo.toml Cargo.lock Justfile config/Dockerfile src/lib.rs src/main.rs src/static_app.rs src/api_v1_tests.rs web/investigate
git commit -m "feat: serve investigation workspace shell"
```

Do not close `.2` until browser tests in Task 7 pass.

## Task 4: Safe `/api/v1/graph/*` Routes And Evidence Hydration

**Files:**
- Modify: `src/api_v1.rs`
- Modify: `src/api_v1_tests.rs`
- Modify: `src/app/services/investigation.rs`
- Modify: `src/app/services/investigation_tests.rs`

**Interfaces:**
- Consumes: existing graph service methods and Task 1 `AppGraph*` DTOs.
- Produces:
  - `GET /api/v1/graph/entity`
  - `GET /api/v1/graph/around`
  - `GET /api/v1/graph/explain`
  - `GET /api/v1/graph/evidence`
  - optional bounded batch evidence endpoint only if the UI needs batch hydration.

- [ ] **Step 1: Write failing denylist tests for graph routes**

Tests must seed graph evidence and assert serialized `/api/v1/graph/*` responses do not contain:

```text
source_id
source_signature_hash
metadata_path
metadata_json
raw_frame
ai_transcript_path
cache_path
source_path
relationship_key
```

Also assert success and error responses include `Cache-Control: no-store`.

- [ ] **Step 2: Implement explicit graph conversions**

For every v1 graph route:
- call the existing graph service method
- convert to `AppGraphResponse`, `AppEvidenceSummary`, and `AppEntitySummary`
- preserve projection/truncation/degraded metadata in the envelope
- never return raw graph response models

- [ ] **Step 3: Avoid evidence N+1**

Initial graph responses may include bounded safe evidence samples already returned by existing graph calls. The browser must not call `/api/v1/graph/evidence` for every visible edge on first render.

Implement either:
- lazy single evidence lookup only when the operator selects one evidence id, or
- a bounded batch endpoint accepting at most `max_evidence_rows`.

Add tests proving high-degree first render does not trigger per-edge evidence fanout.

- [ ] **Step 4: Run and commit**

Run:

```bash
cargo test api_v1_tests::api_v1_graph app::services::investigation_tests::graph_ -- --nocapture
```

Expected: PASS.

Commit:

```bash
git add src/api_v1.rs src/api_v1_tests.rs src/app/services/investigation.rs src/app/services/investigation_tests.rs
git commit -m "feat: add safe investigation graph api"
bd close syslog-mcp-6b9tk.1
```

## Task 5: Pressure-First BAM Mode

**Files:**
- Modify: `src/app/services/investigation.rs`
- Modify: `src/app/services/investigation_tests.rs`
- Modify: `src/api_v1.rs`
- Modify: `src/api_v1_tests.rs`

**Interfaces:**
- Consumes: `BamInvestigationRequest`, `fleet_state`, `host_state`, `correlate_state`, `timeline`, `incident_context`, graph cache, and safe DTO helpers.
- Produces:
  - `CortexService::investigation_bam(req: BamInvestigationRequest) -> ServiceResult<InvestigationEnvelope<BamInvestigationResponse>>`
  - `POST /api/v1/investigations/bam`

- [ ] **Step 1: Write failing BAM tests**

Service tests must cover:
- pressure signal before visible failure ranks first
- missing/delayed pressure telemetry returns `pressure_unavailable` and lowers confidence
- service/container behavior cannot become causal when pressure is unavailable
- weak evidence becomes `open_question`
- baseline contrast uses bounded hour/day rollups by default
- retained-out/missing evidence is visible
- budget breaches return partial metadata

- [ ] **Step 2: Implement rollup-backed baseline**

BAM request shape:

```json
{
  "host": "edgehost",
  "from": "2026-06-20T02:10:00Z",
  "to": "2026-06-20T02:20:00Z",
  "baseline_from": "2026-06-19T02:10:00Z",
  "baseline_to": "2026-06-20T02:00:00Z",
  "budget": {}
}
```

Implementation requirements:
- default baseline is bounded and finite; do not scan unbounded 7-day/every-host windows
- use existing rollup-backed timeline behavior for hour/day baselines when possible
- expose rollup staleness/degraded reasons in metadata
- cap timeline buckets and candidate fanout before DB calls

- [ ] **Step 3: Anchor on host pressure before fanout**

Pressure sources in order:
1. `host_state` and `fleet_state` pressure flags
2. `correlate_state` around the failure window
3. bounded log summaries for OOM, memory, load, disk, network, reboot

If pressure data is unavailable, return `pressure_unavailable`, open questions, and no causal service/container claim.

- [ ] **Step 4: Fan out to bounded candidates**

After pressure anchoring, fan out top-K only:
- services/containers/restarts
- log spikes
- AI sessions
- deploy/config clues
- error signatures

Rank by:
- pressure-first ordering
- temporal ordering
- evidence diversity
- graph trust
- confidence
- repeated/escalating signals
- baseline contrast
- weak/missing/ambiguous penalties

- [ ] **Step 5: Add API tests and run**

Route tests must prove:
- missing/wrong bearer returns `401`, not HTML
- success/error return `no-store`
- malformed windows return `400`
- broad windows are clamped and visible in metadata

Run:

```bash
cargo test app::services::investigation_tests::bam_ api_v1_tests::api_v1_bam -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit and close `.4`**

```bash
bd update syslog-mcp-6b9tk.4 --claim
git add src/app/services/investigation.rs src/app/services/investigation_tests.rs src/api_v1.rs src/api_v1_tests.rs
git commit -m "feat: add pressure-first bam investigation workflow"
bd close syslog-mcp-6b9tk.4
```

## Task 6: Graph Canvas, Evidence Panel, And Render Budgets

**Files:**
- Create: `web/investigate/src/graph.ts`
- Modify: `web/investigate/src/types.ts`
- Modify: `web/investigate/src/render.ts`
- Modify: `web/investigate/src/main.ts`
- Modify: `web/investigate/src/state.ts`
- Modify: `web/investigate/src/render.test.ts`
- Modify: `web/investigate/src/styles.css`

**Interfaces:**
- Consumes: Task 4 safe graph DTOs and Task 5 BAM/Ask responses.
- Produces:
  - Cytoscape graph rendering with caps
  - graph render debouncing
  - stale fetch/layout cancellation
  - no re-layout when graph ids are unchanged
  - evidence panel with missing-source/retained-out states

- [ ] **Step 1: Add frontend tests**

Vitest tests must prove:
- graph labels/tooltips/log excerpts use text nodes
- retained-out evidence is distinct from no evidence
- metadata-only degraded state renders when arrays are empty
- auth failures show visible state without token echo
- graph render skips re-layout when graph node/edge ids are unchanged

- [ ] **Step 2: Implement Cytoscape renderer**

Renderer requirements:
- max nodes/edges from server caps, not hardcoded client guesses
- hidden counts visible
- `cose` layout only for bounded graphs
- debounce expansion/select requests
- abort stale fetches
- destroy old graph only after new data is ready
- layout timeout/fallback state if render exceeds budget

- [ ] **Step 3: Wire Ask/BAM/Graph UI**

UI regions:
- Ask bar with mode selector
- Answer stack
- Graph canvas
- Evidence panel
- Timeline/log strip
- Metadata/degraded state banner

No marketing/landing page. First screen is the actual workspace.

- [ ] **Step 4: Run and commit**

Run:

```bash
npm test --prefix web/investigate
npm run build --prefix web/investigate
```

Expected: PASS.

Commit:

```bash
bd update syslog-mcp-6b9tk.5 --claim
git add web/investigate
git commit -m "feat: render bounded investigation graph"
bd close syslog-mcp-6b9tk.5
```

## Task 7: End-To-End Verification, Concurrency, Docs, And Release

**Files:**
- Create: `web/investigate/playwright.config.ts`
- Create: `web/investigate/tests/workspace.spec.ts`
- Modify: `src/api_v1_tests.rs`
- Modify: `docs/api.md`
- Modify: `docs/INVENTORY.md`
- Modify: `CHANGELOG.md`
- Modify: version-bearing files via `cargo xtask bump-version minor`

**Interfaces:**
- Consumes: all prior tasks.
- Produces deterministic backend/browser/security/performance proof and a draft PR.

- [ ] **Step 1: Add Playwright browser tests**

Browser tests must cover:
- `/app/investigate` loads nonblank at desktop and narrow widths
- token entry is memory-only and clearable
- Ask happy path renders answer, metadata, graph region, evidence region, and logs
- BAM happy path renders pressure state
- hostile graph labels/log excerpts/evidence text do not execute HTML/script
- graph canvas is nonblank for high-degree fixture and does not overlap panels
- degraded/truncated/partial/auth-failed/version-skew states are visible

- [ ] **Step 2: Add backend verification tests**

Backend tests must cover:
- `/api/v1` auth after static app merge
- `no-store` on `/api/v1` success and error
- CSP/no-store on app shell
- `/app/*` fallback does not intercept `/api/*`, `/api/v1/*`, `/mcp`, `/health`, or `/v1/logs`
- high-degree graph payload stays within server caps
- BAM baseline uses bounded rollup-compatible buckets
- prompt injection cannot alter claim semantics
- forbidden fields never appear in serialized browser responses

- [ ] **Step 3: Add starvation/concurrency tests**

Add tests proving concurrent broad investigations do not starve:
- `/health`
- a cheap `/api/*` read such as `/api/version` or `/api/hosts`
- ingest writer access or the writer-reserved connection invariant

The expected behavior can be success or explicit bounded busy/partial metadata, but not silent hangs.

- [ ] **Step 4: Run local server and browser tests**

Start a long-lived server:

```bash
CORTEX_HOST=0.0.0.0 CORTEX_API_TOKEN=secret CORTEX_DB_PATH=target/investigation-e2e/cortex.db cargo run -- serve mcp
```

Run browser tests:

```bash
CORTEX_E2E_BASE_URL=http://127.0.0.1:3100 npm run test:browser --prefix web/investigate
```

Expected: PASS on desktop and narrow viewport projects.

- [ ] **Step 5: Update docs**

Update `docs/api.md` with:
- `/api/v1/investigations/ask`
- `/api/v1/investigations/bam`
- safe `/api/v1/graph/*`
- auth and cache behavior
- claim type semantics

Update `docs/INVENTORY.md` with:
- `/app/investigate`
- `/api/v1/investigations/ask`
- `/api/v1/investigations/bam`
- safe graph endpoints

- [ ] **Step 6: Version bump and gates**

Run:

```bash
cargo xtask bump-version minor
cargo xtask check-version-sync
cargo xtask check-release-versions
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo check
npm test --prefix web/investigate
npm run build --prefix web/investigate
```

Expected: all PASS.

- [ ] **Step 7: Commit, close beads, push, PR**

```bash
bd update syslog-mcp-6b9tk.6 --claim
git add src web docs Cargo.toml Cargo.lock server.json mcpb/manifest.json docker-compose.prod.yml Justfile config/Dockerfile CHANGELOG.md
git commit -m "feat: verify investigation workspace end to end"
bd close syslog-mcp-6b9tk.2
bd close syslog-mcp-6b9tk.3
bd close syslog-mcp-6b9tk.6
bd close syslog-mcp-6b9tk
git pull --rebase
bd dolt push
git push -u origin "$(git branch --show-current)"
gh pr create --draft --base main --head "$(git branch --show-current)" --title "[codex] implement graph investigation workspace" --body-file docs/superpowers/plans/2026-06-20-graph-investigation-workspace.md
```

Expected: draft PR opens for the implementation branch.

## Failure Modes Table

| Codepath | Failure Mode | Rescued? | Test? | User Sees? | Logged? |
| --- | --- | --- | --- | --- | --- |
| `/api/v1/graph/*` | Raw graph fields leak source metadata | Yes via App DTO conversion | Yes denylist tests | No leak; safe response | Yes on conversion errors |
| `/api/v1/investigations/ask` | Broad/high-degree prompt exhausts read permits | Yes via gates/timeouts/concurrency tests | Yes | Partial state | Yes |
| `/api/v1/investigations/ask` | FTS syntax error from natural prompt | Yes via sanitized/fallback search | Yes | Open question/partial | Yes |
| `/api/v1/investigations/bam` | Missing heartbeat data misread as no pressure | Yes via `pressure_unavailable` | Yes | Visible degraded state | Yes |
| `/app/investigate` | Vite assets 404 in Docker | Yes via asset route/Docker smoke | Yes | App error, not blank | Yes |
| Browser token client | Token persisted or echoed | Yes via memory-only/clear/no echo tests | Yes | Clear auth state | No token logged |
| Graph renderer | Layout blocks UI on high-degree graph | Yes via caps/debounce/render budget | Yes | Degraded graph state | Yes |
| Static fallback | API typo returns HTML | Yes via full composed-router tests | Yes | 401/404 JSON as appropriate | Yes |

No row has `Rescued = No`, `Test = No`, and silent user impact.

## Deferrable Work

- Saved investigation cases, annotations, collaboration, and export.
- Broad `/api/v1` parity for every existing `/api/*` route.
- Full natural-language understanding beyond entity/time extraction.
- LLM-powered narrative generation.
- Advanced causal scoring beyond conservative evidence-ranked claims.
- Cross-session persistent investigation caches.
- Same-origin authenticated app session beyond memory-only bearer entry.
- Live production-data E2E before deterministic hostile and scale fixtures.
- Browser graph workers and richer layout algorithms, as long as v1 has strict caps, debounce, cancellation, and layout timeout.

## Self-Review

Spec coverage:
- `/api/v1`, forced auth, safe DTOs, no-store, and `/v1/logs` preservation are covered by Tasks 1, 2, and 4.
- Embedded SPA, explicit browser auth, scoped `/app/*` fallback, pinned bundled graph assets, CSP, Vite asset serving, and Docker asset availability are covered by Task 3.
- Ask + Explain server-side orchestration, conservative claim types, request budgets, open-question ambiguity handling, and prompt-injection semantics are covered by Task 2.
- Pressure-first BAM Mode, bounded baseline, heartbeat/fleet/correlate-state pressure anchoring, top-K fanout, and rollup-backed baseline behavior are covered by Task 5.
- Evidence/trust/degraded/security states, redaction, hostile text rendering, retained-out evidence, graph caps, and stale-request cancellation are covered by Tasks 4 and 6.
- Browser/API verification, fallback isolation, concurrency/starvation, high-degree fixtures, hostile payload fixtures, version bump, and docs are covered by Task 7.

Placeholder scan:
- No unresolved placeholder markers or vague test instructions remain.

Type consistency:
- Rust DTOs from Task 1 are consumed by Tasks 2, 4, and 5.
- TypeScript UI types are narrow views of the Rust browser DTOs and do not mirror raw internal graph/service types.
- API paths are consistently `/api/v1/investigations/ask`, `/api/v1/investigations/bam`, `/api/v1/graph/entity`, `/api/v1/graph/around`, `/api/v1/graph/explain`, and `/api/v1/graph/evidence`.
