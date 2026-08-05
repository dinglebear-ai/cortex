# RAG over Historical Incidents and AI Sessions

**Epic:** `syslog-mcp-h6da`
**Author:** design spec
**Status:** Draft (depends on Epic B `syslog-mcp-1wjr` — Enrichment Framework)
**Date:** 2026-05-16

## 1. Goal & Non-Goals

### Goal

Give an AI agent (or the operator) the ability to ask, of any fresh log/error/anomaly: **"have we seen this before? what fixed it?"** — and get back a ranked list of structurally similar past incidents plus the AI debugging sessions that resolved them, optionally synthesized into a narrative answer with citations.

Concrete user-facing capabilities:

- `similar_incidents` — given a log id, time window, or freeform description, return ranked past incidents (structurally similar, not just keyword-similar) + correlated AI sessions.
- `ask_history` — natural-language question over the incident corpus with LLM synthesis, grounded in retrieved incident cards and session transcripts.
- `suggest_fix` — given a current incident signature, surface the resolution narrative from any prior AI session that closed the same shape of problem.

### Non-Goals

- **Reinventing embedding/retrieval.** axon is already operational on this host (Qdrant + dense + BM42 sparse, hybrid via RRF, LLM synthesis via `ask`). We use it as the substrate.
- **Replacing FTS5 keyword search.** `cortex search` stays the fast path for known strings. RAG is the *semantic* and *narrative* lane.
- **Realtime alerting.** This epic does not page anyone. Retrieval is on-demand from the MCP tool surface.
- **Multi-tenancy / per-user vector partitioning.** Single-operator homelab; one Qdrant collection.
- **Embedding raw transcripts a second time.** Mnemo already indexes AI sessions in SQLite with FTS5; we *correlate* with mnemo, we do not duplicate transcripts into Qdrant.

## 2. Architecture

```text
INGEST PATH (write side, idempotent)
────────────────────────────────────

  cortex UDP/TCP/Docker ──► syslog parser ──► enrichment (Epic B fields)
                                                       │
                                                       ▼
                                              ┌─────────────────┐
                                              │ runtime mpsc    │
                                              │ batch writer    │
                                              └────────┬────────┘
                                                       ▼
                                              SQLite logs + ai_*
                                                       │
                                                       ▼
                                         ┌────────────────────────┐
                                         │  incident extractor    │  (new)
                                         │  ─ trigger detector    │
                                         │  ─ window collector    │
                                         │  ─ signature hasher    │
                                         │  ─ card renderer       │
                                         └───────────┬────────────┘
                                                     ▼
                                          incidents (SQLite)        ◄─┐
                                                     │                │
                                                     ▼                │
                                          scrub + render card        │
                                                     │                │
                                                     ▼                │
                                   axon embed (action=embed) ─► Qdrant│
                                                                      │
                                          backfill / reindex ─────────┘


QUERY PATH (read side)
──────────────────────

  MCP caller ──► similar_incidents / ask_history / suggest_fix
                          │
                          ▼
                signature builder
            (from log id | window | freeform)
                          │
                          ▼
                ┌────────────────────┐       ┌─────────────────────┐
                │ axon query (dense  │       │ mnemo search_ai_    │
                │  + BM42 hybrid)    │       │ sessions (FTS5)     │
                │  filter: incidents │       │ on incident keywords│
                │  collection        │       │ + host + time hint  │
                └─────────┬──────────┘       └──────────┬──────────┘
                          ▼                              ▼
                    incident hits                  session hits
                          │                              │
                          └──────────────┬───────────────┘
                                         ▼
                                  fuse + rerank
                            (RRF + recency + host match)
                                         ▼
                              (optional) axon ask
                            for LLM synthesis with citations
                                         ▼
                                  MCP response
```

## 3. Incident Unit Definition

**Decision: time-windowed cluster, seeded by a trigger event, deduped by template fingerprint.** Hybrid of all three options, justified below.

### Why not single-event

A single OOM kernel line is one row, but it is rarely diagnostic on its own — the lines immediately before it (`Out of memory: Killed process 12345 (foo)`, the call trace, the dmesg context) carry the actual signal. Embedding rows in isolation dilutes the signal and explodes Qdrant cardinality (4.9M rows today).

### Why not pure pattern cluster

`src/db/analytics.rs::patterns` already clusters near-duplicates via `normalize_template` (IPs, UUIDs, hex, digit runs → placeholders). That's the right *deduplication* primitive but the wrong *retrieval unit*: a pattern is "this template has been seen 3,402 times across 7 hosts" — useful for aggregates, not for "what happened on jenny last Thursday at 03:17."

### The chosen unit: **Incident**

An **incident** is a tuple `(signature_hash, host, time_window)` materialized as a row in a new `incidents` SQLite table, with:

1. **Trigger event.** One of a curated set of structured signals from Epic B:
   - `kernel.oom` (any OOM-killer line)
   - `docker_event` with `action in {die, oom, kill, restart_loop>=3}`
   - `authelia` with `result in {denied, lockout}` (≥N within window)
   - `fail2ban` ban-list change
   - `swag` 5xx burst (status≥500 count > threshold)
   - `adguard` block-rate spike
   - severity ≤ `err` from any host where Epic B classifies the source
2. **Window.** Default ±60s around the trigger, expandable per source (OOM → ±120s to capture call trace; fail2ban → coalesce all bans within 300s into one incident).
3. **Signature hash.** Stable hash over the *normalized template of the trigger line plus the structured fields populated by Epic B*. Example for OOM:
   ```
   sha256(app_name=kernel | event=oom | victim=<process> | host=<host>)
   ```
   The host is part of the signature for `similar_incidents` filtering but *not* for cross-host pattern matching — we keep both `signature_hash` (with host) and `signature_hash_xhost` (host stripped) to support both queries.
4. **Boundary rules.**
   - **Start:** first trigger event with no open incident of the same `signature_hash_xhost` in the last `window_close` seconds on that host.
   - **End:** no further trigger event for the same signature within `window_close` (default 300s). On close, the incident is finalized, the card is rendered, scrubbed, and sent to axon embed.
   - **Coalescing:** within an open window, additional matching triggers append to the same incident (bumping `event_count`, extending `last_seen`).
   - **Max duration cap:** 1 hour. Long-running flapping (e.g. a container in restart-loop for a day) gets sliced into hourly incidents.
5. **Pattern reuse.** The signature builder calls `normalize_template` from `src/db/analytics.rs` on the raw trigger message before hashing, so we get the existing battle-tested normalization (UUIDs, IPv4, hex, digit runs).

This unit is **dense enough** to embed meaningfully (a single rich card with surrounding context) and **bounded enough** to fit in Qdrant at sane vector counts (see §11).

## 4. Embedding Text Template

What gets embedded is a **templated incident card** — structured text combining Epic B's typed fields, a normalized signature line, and 3–5 sample raw lines for grounding. Templated cards consistently beat raw-text RAG when the source is noisy log data; Pinecone's "metadata-aware embeddings" guide and the LlamaIndex `MetadataExtractor` post both observe this, as do production write-ups from Honeycomb's "BubbleUp" (which embeds a *summary description* per anomaly cluster rather than raw events). The intuition: an embedding model trained on natural English does better with a sentence than with `kernel: [12345.678] Out of memory: Killed process 9876 (containerd-shim) total-vm:...`.

### Template

```text
INCIDENT {incident_id}
host={host} app={app_name} source={epic_b_source}
window={t_start}..{t_end} ({duration_s}s) event_count={n}
signature: {normalized_signature_line}

structured:
{key1}={val1}
{key2}={val2}
...

sample lines:
- {raw_line_1}
- {raw_line_2}
- {raw_line_3}
```

### Worked Example A — Kernel OOM

```text
INCIDENT inc_2026-05-15T03:17:12Z_jenny_a4f9
host=jenny app=kernel source=kernel.oom
window=2026-05-15T03:17:08Z..2026-05-15T03:17:34Z (26s) event_count=4
signature: kernel: Out of memory: Killed process <n> (<process>) total-vm:<n>kB

structured:
oom_victim_comm=plex
oom_victim_pid=18422
oom_killer_score=987
mem_total_kb=33554432
mem_free_kb=412
cgroup=/docker/abc123
container_name=plex

sample lines:
- kernel: [12459.119] Out of memory: Killed process 18422 (plex) total-vm:8421492kB
- kernel: [12459.121] oom_reaper: reaped process 18422 (plex), now anon-rss:0kB
- kernel: [12459.198] plex invoked oom-killer: gfp_mask=0x100cca, order=0, oom_score_adj=0
```

### Worked Example B — Docker container die

```text
INCIDENT inc_2026-05-15T14:02:03Z_squirts_b7e2
host=edgehost app=dockerd source=docker_event
window=2026-05-15T14:02:00Z..2026-05-15T14:02:15Z (15s) event_count=2
signature: docker_event action=die container=<name> exitCode=<n>

structured:
event_action=die
container_name=qbittorrent
container_image=lscr.io/linuxserver/qbittorrent:latest
exit_code=137
prev_action=oom
restart_policy=unless-stopped

sample lines:
- container die abc123def456 (image=lscr.io/linuxserver/qbittorrent:latest, name=qbittorrent, exitCode=137)
- container oom abc123def456 (image=lscr.io/linuxserver/qbittorrent:latest, name=qbittorrent)
```

### Worked Example C — Fail2ban cluster

```text
INCIDENT inc_2026-05-15T07:44:00Z_tootie_c1a3
host=nashost app=fail2ban source=fail2ban
window=2026-05-15T07:44:00Z..2026-05-15T07:48:51Z (291s) event_count=14
signature: fail2ban Ban <ip> jail=<jail>

structured:
jail=sshd
banned_ip_count=14
unique_source_asns=3
top_country=CN
attack_window_s=291
total_failed_logins=147

sample lines:
- Ban 203.0.113.45 (jail=sshd, 11 failures in 60s)
- Ban 198.51.100.12 (jail=sshd, 9 failures in 45s)
- Found 203.0.113.45 (matches=11)
```

## 5. Embedding Pipeline

### When to embed: **on incident close**, not on every event

A new background task in `RuntimeCore` ("incident_finalizer") wakes every `tick_interval` (default 30s) and:

1. Reads open incidents from the `incidents` table where `last_seen < now - window_close`.
2. Renders the card via the §4 template.
3. Runs the **same** `scrub_ai_message` from `src/syslog/enrichment.rs` over the card (see §9).
4. Writes the card to a staging path: `/data/incidents/{incident_id}.md`.
5. Calls axon: `action=embed, subaction=start, input=/data/incidents/{incident_id}.md`.
6. Stores the returned `job_id` on the incident row. Marks `embed_status=pending`.
7. A second loop polls `action=embed, subaction=status, job_id=...` until terminal and updates `embed_status` to `embedded` or `failed`.

**Why on-close, not live-tap.** Live-tap creates a race where an OOM's call-trace lines arrive after the trigger; we'd embed an incomplete card. On-close with a 300s coalesce window guarantees the card is whole. Latency cost: incidents are queryable ~5 minutes after they happen, which is acceptable for "have we seen this before."

**Why not nightly batch.** Batch hides incidents until the next morning; the operator wants "Claude, look up that OOM from 20 minutes ago" to work.

### Idempotency

The Qdrant payload includes `incident_id` (UUIDv7 of the syslog-mcp incident). Before embedding, the finalizer queries axon `action=query` with a filter on `incident_id` — if present, skip. On re-embed (signature drift, new fields, resolution annotation appended — see §10), delete by `incident_id` first via axon's per-URL retrieve+delete is not a primitive; instead we use the `source_type: incident` payload field plus the `incident_id` as the canonical key, and rely on overwriting the staging file with a stable path `/data/incidents/{incident_id}.md` so axon's URL-keyed dedupe kicks in.

### Failure handling

- Axon unreachable → leave `embed_status=pending`, retry with exponential backoff (1m, 5m, 30m, capped at 1h).
- Repeated failure (>5 attempts) → `embed_status=failed`, log warning, surface in `cortex dr` health check, do not block ingest.
- Storage budget exceeded (`maintenance.rs` guardrail trips) → pause embedding writes but continue closing incidents in SQLite; resume when budget recovers.

## 6. Storage

### `incidents` table (SQLite)

```sql
CREATE TABLE incidents (
    incident_id           TEXT PRIMARY KEY,        -- UUIDv7
    signature_hash        TEXT NOT NULL,           -- host-scoped
    signature_hash_xhost  TEXT NOT NULL,           -- host-stripped
    hostname              TEXT NOT NULL,
    app_name              TEXT NOT NULL,
    source                TEXT NOT NULL,           -- 'kernel.oom' | 'docker_event' | ...
    severity              TEXT,
    first_seen            TEXT NOT NULL,           -- ISO 8601
    last_seen             TEXT NOT NULL,
    event_count           INTEGER NOT NULL,
    sample_log_ids        TEXT NOT NULL,           -- JSON array of logs.rowid
    structured_fields     TEXT NOT NULL,           -- JSON, source-shaped
    card_path             TEXT,                    -- /data/incidents/<id>.md
    embed_status          TEXT NOT NULL DEFAULT 'pending',  -- pending|embedded|failed
    embed_job_id          TEXT,
    embed_attempts        INTEGER NOT NULL DEFAULT 0,
    resolution_session_id TEXT,                    -- link to mnemo session that fixed it
    resolution_notes      TEXT,                    -- freeform, from suggest_fix feedback
    closed                INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_incidents_sig_xhost ON incidents(signature_hash_xhost, last_seen DESC);
CREATE INDEX idx_incidents_host_time ON incidents(hostname, last_seen DESC);
CREATE INDEX idx_incidents_embed_status ON incidents(embed_status) WHERE embed_status != 'embedded';
```

### What lives where

| Data | SQLite (`incidents`) | Qdrant payload |
|---|---|---|
| `incident_id` | PK | `incident_id` |
| `signature_hash` / `_xhost` | yes | yes (for filtering) |
| `hostname`, `app_name`, `source` | yes | yes |
| `first_seen`, `last_seen` | yes | `first_seen`, `last_seen` |
| `event_count` | yes | yes |
| `structured_fields` (JSON blob) | full | flattened top-level keys for filterable ones (victim_comm, container_name, jail, exit_code) |
| `sample_log_ids` | yes | no (resolve via syslog-mcp on demand) |
| Raw card text | `card_path` filesystem | embedded vector + payload `text` |
| Resolution link | `resolution_session_id` | refreshed on re-embed |

SQLite remains the source of truth. Qdrant is a *derived* index — if it goes away, we can rebuild from `incidents` + `card_path`.

## 7. Retrieval Pipeline

### 7.1 Query construction

Three entry points, all converging on a single internal `IncidentQuery`:

- **By log id** (`similar_incidents(log_id=...)`): load the log row, attempt to find a containing incident (`incidents` where `log_id ∈ sample_log_ids`); if found, build the query from that incident's card; if not, synthesize a one-off card from the log + ±60s context and use that.
- **By time window** (`similar_incidents(host=..., from=..., to=...)`): find incidents in the window and use the densest one (highest `event_count × severity_weight`) as the query seed.
- **By freeform text** (`similar_incidents(query="qbittorrent keeps OOMing")` or `ask_history(query=...)`): use the text directly as the axon query; no card synthesis needed.

### 7.2 axon hybrid query parameters

```yaml
action: query
query: <card_text_or_freeform>
collection: <default>           # one shared collection, filtered by source_type
hybrid_search: true             # dense + BM42 sparse with RRF (axon default)
limit: 20                       # over-fetch for rerank
since: optional, default 90d    # bias toward recent
```

We filter results client-side on `payload.source_type == "incident"` (until axon exposes Qdrant payload filter passthrough — open question §13). Score threshold: drop hits with `score < 0.35` after axon's RRF normalization, calibrated against the gold-standard set in §12.

### 7.3 mnemo AI-session correlation

In parallel with the axon call, run `search_ai_sessions` from `src/db/queries.rs`:

```rust
SearchAiSessionsParams {
    fts_query: <keywords extracted from card: container_name, victim_comm, jail, etc.>,
    project: optional,
    from: 180d_ago,             # AI sessions have longer relevance horizon
    limit: 20,
}
```

Keyword extraction: pull the high-signal structured fields (`victim_comm`, `container_name`, `jail`, error_code) and join with `OR`. Avoid stop-words and hostname-only queries (too noisy).

### 7.4 Fuse + rerank

Two ranked lists (incidents from axon, sessions from mnemo) merged via Reciprocal Rank Fusion (k=60, standard) with weights:

- Score component: `1 / (60 + rank)` per list
- Recency boost: `+0.1 * exp(-age_days / 30)` — incidents from a week ago beat incidents from a year ago at equal semantic distance.
- Host-match boost: `+0.15` when the candidate's `hostname` matches the query's hostname (homelab insight: same host, same hardware, same misconfig).
- Resolution boost: `+0.20` when the incident has a non-null `resolution_session_id` (we *especially* want known-fixed cases for `suggest_fix`).

Top-K after rerank: 5 incidents + 3 sessions returned by default; configurable.

### 7.5 LLM synthesis trigger

| MCP action | Synthesis behavior |
|---|---|
| `similar_incidents` | **No** synthesis. Returns ranked structured hits. Cheap and deterministic. |
| `ask_history` | **Yes**, always. Wraps `axon ask` with the assembled context window (top incidents' cards + top session excerpts). |
| `suggest_fix` | **Yes**, but only if at least one hit has `resolution_session_id`. Otherwise return ranked hits with a flag `synthesized: false, reason: "no resolved priors"`. |

When calling `axon ask`, we pass the user's natural-language question as `query` and rely on axon's retrieval — but we *also* supply the incident card text in the prompt context via a thin wrapper (see §8.2 implementation note).

## 8. MCP Action Surface

All three actions live in `src/mcp/tools.rs` alongside existing handlers; service-layer logic in a new `src/app/rag.rs` module. JSON Schemas added to `src/mcp/schemas.rs`.

### 8.1 `similar_incidents`

**Request:**

```json
{
  "action": "similar_incidents",
  "log_id": 1234567,                   // one of these three is required
  "time_window": {"host": "jenny", "from": "2026-05-15T03:00:00Z", "to": "2026-05-15T04:00:00Z"},
  "query": "qbittorrent OOM",
  "limit": 5,
  "include_sessions": true,            // default true
  "since": "180d",                     // default 90d
  "host_filter": "jenny",              // optional, restrict hits to host
  "min_score": 0.35
}
```

**Response:**

```json
{
  "query_card": "INCIDENT seed ...",
  "incidents": [
    {
      "incident_id": "...",
      "score": 0.74,
      "hostname": "jenny",
      "app_name": "kernel",
      "source": "kernel.oom",
      "first_seen": "...",
      "last_seen": "...",
      "event_count": 4,
      "structured_fields": {"oom_victim_comm": "plex", ...},
      "sample_log_ids": [...],
      "card_excerpt": "host=jenny app=kernel source=kernel.oom ...",
      "resolution_session_id": "..."   // null if unresolved
    }
  ],
  "sessions": [
    {
      "session_id": "...",
      "project": "homelab-ops",
      "started_at": "...",
      "snippet": "user: plex keeps getting OOM-killed | assistant: looks like swap is exhausted, let's check...",
      "tool": "claude-code"
    }
  ],
  "diagnostics": {"axon_hits": 12, "mnemo_hits": 7, "rerank_ms": 14}
}
```

### 8.2 `ask_history`

**Request:**

```json
{
  "action": "ask_history",
  "query": "what causes qbittorrent to keep dying on edgehost?",
  "since": "180d",
  "host_filter": "edgehost",
  "max_context_incidents": 8,
  "max_context_sessions": 5
}
```

**Response:**

```json
{
  "answer": "qbittorrent on edgehost has died 14 times in the last 90 days...",
  "citations": [
    {"type": "incident", "incident_id": "...", "score": 0.72},
    {"type": "session",  "session_id":  "...", "score": 0.65}
  ],
  "axon_job_id": "...",
  "diagnostics": {...}
}
```

Implementation: build the retrieval ourselves (we trust our rerank more than axon's defaults), assemble the context, then call `axon ask` with `query` set to the user question and rely on axon's synthesis. If axon ask doesn't accept user-supplied context (its retrieval is internal), we fall back to running our retrieval and feeding the user-question into `axon ask` directly — axon will do its own retrieval over the same collection and reach roughly the same incidents. We accept that small loss in exchange for not reinventing synthesis.

### 8.3 `suggest_fix`

**Request:**

```json
{
  "action": "suggest_fix",
  "incident_id": "...",                // OR log_id, OR query
  "min_resolved_priors": 1
}
```

**Response:**

```json
{
  "synthesized": true,
  "suggestion": "Last time this happened (2026-04-12, jenny, plex OOM), you increased the docker memory limit to 4G and added oom_score_adj=-500. After that, no recurrences for 33 days.",
  "based_on": [
    {"incident_id": "...", "resolution_session_id": "...", "session_excerpt": "..."}
  ],
  "alternatives": [
    {"incident_id": "...", "summary": "different fix tried — restart docker daemon, didn't stick"}
  ]
}
```

If no resolved priors exist, return `synthesized: false` with the raw ranked-hits payload, so the caller can still see *something*.

### 8.4 `mark_incident_resolved` — closing the loop

Promoted to a first-class action (bead `syslog-mcp-q22k`). Pinned in `docs/contracts/mcp-actions.md` alongside the other 3 F-epic actions.

```json
{"action": "mark_incident_resolved",
 "incident_id": "...",
 "session_id": "...",
 "notes": "increased mem limit",
 "reopen": false}
```

Sets `resolution_session_id` and `resolution_notes` on the `incidents` row, flips `resolution_present` to `1`, and enqueues a re-embed so the new resolution lives in the Qdrant payload too. `reopen: true` clears the resolution state when a "fixed" incident reappears. This is what makes `suggest_fix` get smarter over time — without it, the corpus only contains problems, not solutions.

Action surface = **4 total**: `similar_incidents`, `ask_history`, `suggest_fix`, `mark_incident_resolved`.

## 9. Privacy & Redaction

Three guards, layered:

1. **Reuse `scrub_ai_message`.** The existing scrubber in `src/syslog/enrichment.rs` (regex for `password=`, `api[_-]?key=`, `secret=`, plus user-configured `api_token`) runs over every card before it leaves the host. Today it gates on `app_name` being an AI source; we lift that gate for cards so it runs unconditionally.
2. **Strict allowlist on structured fields.** Each Epic B source declares which structured keys are safe to emit; we *never* serialize the full structured_fields blob blindly into Qdrant. New sources must be onboarded with an explicit allowlist (`docker_ingest: [event_action, container_name, container_image, exit_code]`).
3. **Pre-embed secrets scan.** Before the `axon embed` call, run a final `trufflehog`-style regex sweep (private keys, AWS keys, JWT, generic high-entropy strings >40 chars in `key=value` shape) over the card. Any match → quarantine the card to `/data/incidents/_quarantined/`, log a warning with the field name, do not embed. Surfaces in `cortex dr`.

The card never embeds `source_ip` for inbound auth attacks (already redacted to `/24` upstream in fail2ban enrichment), and never embeds full user-agent strings (they're high-cardinality and leak browser fingerprints).

## 10. Freshness & Reindex

Triggers that cause a card to be re-embedded:

- **Resolution added** (`mark_incident_resolved`). High value — adds the fix narrative.
- **Signature drift detected.** A nightly job scans incidents whose `signature_hash_xhost` clusters have grown since last embed (e.g. >2x event_count) and re-embeds the densest representative to refresh `event_count` and `last_seen` in the payload.
- **Epic B schema bump.** If a source's allowlist grows (new structured field added), backfill: re-render and re-embed all incidents from that source in the last 90 days. Older than 90d is left alone — diminishing returns.

Idempotency relies on stable `card_path` per `incident_id`; overwriting the file and re-embedding causes axon to dedupe by URL.

## 11. Storage Projection

**Inputs (from production memory + axon stats):**
- Current SQLite logs: ~4.9M rows.
- Ingest rate (rough est from compose/ingest_rate analytics): ~0.5–2 events/s steady-state, bursts to ~20/s.
- Axon Qdrant collection today: 1,663,770 points across 113,898 docs (~14.6 chunks/doc avg).

**Incident rate estimate:** Triggers are *narrow* — OOMs, container dies, fail2ban bans, severity-err+ from typed sources. From the existing `errors` action's typical 24h count on this homelab (~50–200 err+/day across all hosts), and accounting for the coalescing window collapsing bursts, we expect **30–150 incidents/day**, call it 100/day midpoint.

**Chunks per incident:** Cards are short — ~300–800 tokens. axon's chunker at default settings yields 1–2 chunks per card. Call it 1.5.

**30-day projection:** 100 incidents/day × 30 days × 1.5 chunks = **4,500 vectors/month**. Annualized: ~55K vectors/year. Negligible vs axon's existing 1.66M points (+0.27%/month). Storage and recall impact: zero concern at this scale; the BM42 sparse index and HNSW graph won't notice.

**SQLite incidents table:** ~100 rows/day × 30 days × ~2KB/row (with JSON structured_fields) = **6 MB/month**, **72 MB/year**. Well under the storage guardrail (`maintenance.rs` default cap).

**Card files on disk:** 100 cards/day × ~1KB avg = 100 KB/day → 3 MB/month. Trivial.

**Conclusion:** No infra changes required. axon's existing Qdrant deployment absorbs this load forever.

## 12. Test Plan

### Fixture corpus

A `tests/fixtures/incidents/` directory with hand-curated incident families:

- **OOM family:** 6 OOM incidents, 3 victim_comm = `plex`, 3 victim_comm = `qbittorrent`, across 2 hosts.
- **Docker die family:** 8 incidents, mix of `exit_code=137` (oom-killed) and `exit_code=1` (app crash).
- **Fail2ban family:** 5 bursts, sshd jail.
- **Authelia lockout family:** 4 incidents.
- **Distractor set:** 20 unrelated incidents (kernel non-OOM, swag 5xx, adguard) to ensure they don't pollute results.

### Gold standard

For each fixture incident, a hand-annotated set of "correct similar incidents" (typically 3–5 within the same family on similar hosts). Stored as `tests/fixtures/incidents/gold.json`:

```json
{"query_incident": "fix_oom_plex_jenny_01", "expected_hits": ["fix_oom_plex_jenny_02", "fix_oom_plex_squirts_01"], "must_not_hit": ["fix_swag_5xx_01"]}
```

### Metrics

- **Recall@5** target: ≥0.80 on the gold set after the rerank.
- **Precision@5** target: ≥0.60.
- **Latency**: p50 < 500ms, p95 < 2s for `similar_incidents` (no synthesis); p95 < 8s for `ask_history` (synthesis dominated by axon ask).

### Smoke test in CI

`scripts/smoke-test-rag.sh`:

1. Spin up syslog-mcp with a temp DB.
2. Replay the fixture corpus through the ingest pipeline (forces incident extraction).
3. Wait for `embed_status=embedded` on all incidents.
4. Run `similar_incidents` for each fixture query.
5. Compare against gold; fail CI on Recall@5 < 0.75.

Use axon's `action=evaluate` with `retrieval_ab: true` to compare hybrid vs dense-only retrieval on the same gold set during tuning. If hybrid doesn't beat dense by a noticeable margin on our log-card corpus, we set `hybrid_search: false` per-request (the card text has dense semantic content; sparse may add noise).

### Unit-level

- `incident_extractor` tests for boundary rules: trigger detection, window close, coalescing, max-duration slicing.
- `card_renderer` tests for each source's allowlist and template fidelity.
- `redaction` tests: known-secret strings in synthetic cards must trip the pre-embed sweep.

## 13. Open Questions

1. ~~**Payload filter passthrough.**~~ **RESOLVED 2026-05-16.** Verified against `axon://schema/mcp-tool`: `axon ask` and `axon query` accept only `collection`, `since`, `before`, `hybrid_search`, `limit`, `offset` — **no arbitrary payload filters**. Decision: **use a dedicated Qdrant collection `syslog-mcp-incidents`** (separate from axon's default crawl/web collection). All `embed`/`query`/`ask` calls from this epic pass `collection: "syslog-mcp-incidents"`. Coordinate with axon owner on initial collection creation. Client-side payload filtering by host/app is retained for fine-grained queries inside the `similar_incidents` action.
2. ~~**axon ask context injection.**~~ **RESOLVED 2026-05-16.** `axon ask` does its own retrieval and does not accept an injected context block per the verified schema. Implication:
   - `ask_history`: call `axon ask` directly with `collection: "syslog-mcp-incidents"`, `since`/`before` from user, `query` = user's natural-language question. We do NOT supply a pre-assembled context — axon's retrieval drives synthesis. We lose our rerank logic on the synthesis path; accepted in V1.
   - `similar_incidents`: stays deterministic, uses `axon query` (no LLM) + our rerank.
   - `suggest_fix`: two-step. First `axon query` over `syslog-mcp-incidents` (collection-scoped retrieval), then client-side filter to past *resolved* signatures via `payload.resolution_present == true`, then hand-build a synthesis prompt to a separate LLM call (NOT through `axon ask`) so we can include the *current incident card* as primary context plus retrieved past resolutions as references. The synthesis LLM endpoint is configurable; default re-uses any LLM credentials the operator already has wired (Anthropic, OpenAI, or local).
3. **Window tuning per source.** §3 picks defaults (60s default, 120s OOM, 300s fail2ban). These need validation on real homelab data — likely 1-2 iterations after launch.
4. **Cross-host signatures.** Should `similar_incidents` default to `signature_hash` (same host) or `signature_hash_xhost` (any host)? Spec says return both, ranked, but UX may want a knob. Default suggestion: xhost on (homelab operator usually wants "did this ever happen anywhere").
5. **Resolution detection automation.** §8.3 requires the operator to call `mark_incident_resolved`. Can we *infer* a resolution by correlating an incident's `last_seen` with the closest subsequent AI session that mentions the relevant keywords and was followed by a long quiet period for that signature? Worth a follow-up epic; not in this one.
6. **Embedding model choice.** Inherits from axon (TEI-backed). If we discover log-card embeddings underperform vs general English, axon supports model swaps — but that's an axon-side change, out of scope here.
7. **Privacy posture on shared homelabs.** This spec assumes single-operator. If a future use case shares the syslog corpus across multiple users, we need per-user payload filters in Qdrant and a tenancy model — explicitly deferred.

---

**Recommendation status:**
- §3 (incident unit) — **hard pick**, builds on existing `patterns` logic.
- §4 (templated card) — **hard pick**, well-supported by metadata-aware RAG literature.
- §5 (on-close embedding) — **hard pick**, the only sane option given coalescing.
- §7.4 (RRF + boosts) — **recommendation**, exact weights are tunable post-launch.
- §8 (action surface) — **hard pick** for shapes; param defaults tunable.
- §11 (storage) — **calculation**, not a decision; conclusion is robust.
