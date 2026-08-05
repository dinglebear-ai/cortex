---
title: "Incident Card Contract"
created: 2026-05-16
updated: 2026-07-30
---

# Incident Card Contract

**Epic:** `cortex-h6da` (RAG over Historical Incidents and AI Sessions)
**Spec:** [`docs/superpowers/specs/2026-05-16-rag-incidents-design.md`](../superpowers/specs/2026-05-16-rag-incidents-design.md)
**Status:** Historical design contract; not active runtime behavior
**Date:** 2026-05-16

---

## 1. Purpose

This contract defines the exact textual format originally designed for embedding
incident cards into the Qdrant collection `incidents`.

Current production `similar_incidents` is SQLite FTS5-only. It does not embed
incident cards, query Qdrant, or call Axon. Keep this document as design context
for a future semantic RAG implementation; use `docs/mcp/CORRELATION.md` for
current behavior.

Two consumers must agree on this shape:

1. The **card renderer** (write side, in `src/app/rag.rs::render_incident_card`), which converts a finalized `incidents` row plus its source-shaped `structured_fields` blob into the text we ship to `axon embed`.
2. The **retrieval layer** (read side, in `similar_incidents` / `ask_history` / `suggest_fix`), which queries Qdrant for cards and parses payload metadata for client-side filtering.

If the template, the placeholder set, or the payload field set changes, **`schema_version` must bump** and the freshness-window backfill in §7 must run. Embeddings produced under different versions are not commensurable for nearest-neighbor purposes.

This document is the single source of truth. The Tera template in `templates/incident_card.tera` (or wherever it lands) is generated from this spec; the worked examples in §3 are test fixtures.

---

## 2. Card template (canonical)

The renderer uses [Tera](https://keats.github.io/tera/) syntax. The template is intentionally flat — no conditionals beyond the `resolution_present` branch and the `structured_fields` loop — because variation in the embedded text hurts retrieval more than it helps.

```tera
INCIDENT {{ incident_id }}
host={{ host }} app={{ app_name }} source={{ source }}
window={{ first_seen }}..{{ last_seen }} ({{ duration_seconds }}s) event_count={{ event_count }}
severity_max={{ severity_max }}
signature: {{ signature_line }}
signature_hash={{ signature_hash }}
signature_hash_xhost={{ signature_hash_xhost }}

structured:
{%- for kv in structured_fields %}
{{ kv.key }}={{ kv.value }}
{%- endfor %}

sample lines:
{%- for line in sample_lines %}
- {{ line }}
{%- endfor %}
{%- if resolution_present %}

resolution:
{{ resolution_summary }}
{%- endif %}
```

**Required placeholders (the renderer MUST supply all of these; missing values are a renderer bug, not a template bug):**

| Placeholder | Type | Source | Notes |
|---|---|---|---|
| `incident_id` | string (UUIDv7) | `incidents.incident_id` | Stable per incident, never reused. |
| `signature_hash` | string (blake3 hex, 32 chars) | `incidents.signature_hash` | Host-scoped. |
| `signature_hash_xhost` | string (blake3 hex, 32 chars) | `incidents.signature_hash_xhost` | Host stripped — cross-host similarity. |
| `host` | string | `incidents.hostname` | E.g. `dookie`, `jenny`. |
| `app_name` | string | `incidents.app_name` | E.g. `kernel`, `dockerd`, `fail2ban`. |
| `source` | string | `incidents.source` | Epic B source tag, e.g. `kernel.oom`. |
| `first_seen` | string (RFC 3339) | `incidents.first_seen` | UTC, second precision. |
| `last_seen` | string (RFC 3339) | `incidents.last_seen` | UTC, second precision. |
| `duration_seconds` | integer | derived (`last_seen − first_seen`) | 0 for single-event incidents. |
| `event_count` | integer | `incidents.event_count` | ≥1. |
| `severity_max` | string | derived from sample logs | One of `emerg, alert, crit, err, warning, notice, info, debug`. |
| `signature_line` | string | derived | Output of `normalize_template` over the trigger raw message (UUIDs → `<n>`, IPs → `<ip>`, etc.). Single line. |
| `structured_fields` | array of `{key, value}` | `incidents.structured_fields` (JSON) | Already filtered by the per-source allowlist (§4). Sort alphabetically by key for determinism. |
| `sample_lines` | array of strings | derived from `sample_log_ids` | 3–5 lines, post-`scrub_ai_message`. Order matches log timestamp ascending. |
| `resolution_present` | bool | `incidents.resolution_session_id IS NOT NULL` | Controls whether the resolution block renders. |
| `resolution_summary` | string | `incidents.resolution_notes` (or derived from session) | Only included when `resolution_present` is true. Single short paragraph, ≤500 chars. |

**Rendering rules:**

- Trailing newlines stripped — exactly one `\n` at EOF.
- `{%- ... -%}` whitespace control used throughout to avoid blank-line drift between Tera versions.
- All values are emitted verbatim after scrubbing; **no double-escaping**, no JSON encoding inside the card. The card is read by an embedding model, not a parser.
- Empty `structured` and empty `sample lines` are rendered with the section header but no bullets — never omit the header (preserves layout for the embedder).

---

## 3. Three worked examples

These are inline fixtures. `tests/fixtures/incidents/{a,b,c}.md` should match these byte-for-byte once the renderer lands.

### 3.1 Kernel OOM — dookie, Plex container OOM-killed

```
INCIDENT 0192f8a4-7c11-7c2b-9f3d-1e5a4b2c9d01
host=dookie app=kernel source=kernel.oom
window=2026-05-15T02:14:08Z..2026-05-15T02:14:34Z (26s) event_count=4
severity_max=err
signature: kernel: Out of memory: Killed process <n> (<process>) total-vm:<n>kB
signature_hash=8c4f1ae6d3b9742a0e51c8f6b29d4a73
signature_hash_xhost=2f9a17c4e08bd56123c7f4ea88b91d05

structured:
cgroup=/docker/abc123def456
container_name=plex
mem_free_kb=412
mem_total_kb=33554432
oom_killer_score=987
oom_victim_comm=plex
oom_victim_pid=18422

sample lines:
- kernel: [12459.119] plex invoked oom-killer: gfp_mask=0x100cca, order=0, oom_score_adj=0
- kernel: [12459.121] Out of memory: Killed process 18422 (plex) total-vm:8421492kB
- kernel: [12459.198] oom_reaper: reaped process 18422 (plex), now anon-rss:0kB
- kernel: [12459.214] Memory cgroup out of memory: Killed process 18422 (plex) total-vm:8421492kB, anon-rss:7901244kB
```

### 3.2 Docker container die — Authelia exiting with code 137 thrice in 10 min

```
INCIDENT 0192f8a5-1eaa-7c22-8d44-7c2e8f10a3b2
host=tootie app=dockerd source=docker_event
window=2026-05-15T14:02:00Z..2026-05-15T14:11:42Z (582s) event_count=3
severity_max=err
signature: docker_event action=die container=<name> exitCode=<n>
signature_hash=4d12bb74ea90f3185c6d2af80e3b14fa
signature_hash_xhost=9a73c0f2b154ef3680a18bd4f5ca6e21

structured:
container_image=authelia/authelia:4.38
container_name=authelia
event_action=die
exit_code=137
prev_action=oom
restart_policy=unless-stopped
restart_count_10m=3

sample lines:
- container die abc123def456 (image=authelia/authelia:4.38, name=authelia, exitCode=137)
- container die abc123def456 (image=authelia/authelia:4.38, name=authelia, exitCode=137)
- container die abc123def456 (image=authelia/authelia:4.38, name=authelia, exitCode=137)
```

### 3.3 fail2ban ban cluster — sshd jail, multiple IPs in a 5-min window

```
INCIDENT 0192f8a5-9b3c-7c11-a058-3d4e7c1f9a05
host=squirts app=fail2ban source=fail2ban
window=2026-05-15T07:44:00Z..2026-05-15T07:48:51Z (291s) event_count=14
severity_max=notice
signature: fail2ban Ban <ip> jail=<jail>
signature_hash=7fa3c8b15e2d09478b6f1c2a4dd5e603
signature_hash_xhost=c81f4a7b2d9056e3a1b4c8f70e2d9135

structured:
attack_window_s=291
banned_ip_count=14
jail=sshd
top_country=CN
total_failed_logins=147
unique_source_asns=3

sample lines:
- Ban 203.0.113.0/24 (jail=sshd, 11 failures in 60s)
- Ban 198.51.100.0/24 (jail=sshd, 9 failures in 45s)
- Found 203.0.113.0/24 (matches=11)
- Found 198.51.100.0/24 (matches=9)
- Ban 192.0.2.0/24 (jail=sshd, 7 failures in 90s)
```

> Note: source IPs are pre-redacted to `/24` upstream by the fail2ban enrichment (per spec §9), which is why the sample lines show CIDR rather than full addresses.

---

## 4. Embedding rules

1. **Only the rendered card text is embedded.** Payload metadata (§5) lives on the Qdrant point separately; it is *not* concatenated into the card. The card must be self-sufficient as English-ish prose for the embedder, and the payload must be self-sufficient as structured metadata for client-side filtering.

2. **Scrubbing.** `{{ sample_lines }}` and `{{ structured_fields }}` MUST pass through `scrub_ai_message` from `src/syslog/enrichment.rs` *plus* the per-source structured-field allowlist before reaching the template. The allowlist is owned by each Epic B source module — example:
   - `kernel.oom`: `oom_victim_comm`, `oom_victim_pid`, `oom_killer_score`, `mem_total_kb`, `mem_free_kb`, `cgroup`, `container_name`
   - `docker_event`: `event_action`, `container_name`, `container_image`, `exit_code`, `prev_action`, `restart_policy`, `restart_count_10m`
   - `fail2ban`: `jail`, `banned_ip_count`, `unique_source_asns`, `top_country`, `attack_window_s`, `total_failed_logins`
   New sources cannot ship without an explicit allowlist (CI lint enforced).

3. **Length cap.** Each rendered card MUST be ≤ 8192 characters. TEI's typical context window is the binding constraint; oversize cards are silently truncated by the embedder, which corrupts the tail. If the rendered card would exceed 8192 chars, truncate `sample_lines` first (drop from the end until under cap), then `structured_fields` (drop lowest-priority keys as declared by the source allowlist), then hard-clip at 8192 with `...[TRUNCATED]` appended.

4. **Re-embed triggers.** A card is re-rendered and re-embedded when any of the following are true:
   - **Signature drift.** Nightly job detects that the `signature_hash_xhost` cluster's `event_count` has more than doubled since last embed — refresh the densest representative.
   - **Resolution added.** `mark_incident_resolved` was called, populating `resolution_session_id` / `resolution_notes`. High-value: this is what makes `suggest_fix` learn over time.
   - **Epic B schema bump.** A source's allowlist grows (new structured field is now safe to emit). Backfill all incidents of that source from the last 90 days.
   - **`schema_version` bump.** Template or payload layout changed; all incidents in the freshness window (90 days) re-embed.

   Idempotency relies on the stable staging path `/data/incidents/{incident_id}.md` — overwriting the file and re-invoking `axon embed` causes axon to dedupe by URL.

---

## 5. Qdrant payload schema

Every point in `incidents` carries this payload (in addition to the embedded card text, which axon stores in `payload.text` by its own convention):

```json
{
  "incident_id": "0192f8a4-7c11-7c2b-9f3d-1e5a4b2c9d01",
  "signature_hash": "8c4f1ae6d3b9742a0e51c8f6b29d4a73",
  "signature_hash_xhost": "2f9a17c4e08bd56123c7f4ea88b91d05",
  "host": "dookie",
  "app_name": "kernel",
  "source": "kernel.oom",
  "first_seen_ts": 1747276448,
  "last_seen_ts": 1747276474,
  "event_count": 4,
  "severity_max": "err",
  "resolution_present": false,
  "source_type": "incident",
  "schema_version": 1
}
```

| Field | Type | Indexed in Qdrant? | Purpose |
|---|---|---|---|
| `incident_id` | string (UUIDv7) | yes (keyword) | Primary key; idempotency check before embed; cross-reference to SQLite `incidents.incident_id`. |
| `signature_hash` | string (blake3 hex) | **yes** | Client-side filter for "same-host same-shape" recurrence. |
| `signature_hash_xhost` | string (blake3 hex) | **yes** | Client-side filter for cross-host similarity (default for `similar_incidents`). |
| `host` | string | **yes** | Client-side host filter and host-match rerank boost (§7.4 of spec). |
| `app_name` | string | **yes** | Client-side app filter. |
| `source` | string | yes | Epic B source tag (e.g. `kernel.oom`); useful for distractor filtering. |
| `first_seen_ts` | int64 (Unix seconds, UTC) | yes (range) | Time-window filtering and recency boost. |
| `last_seen_ts` | int64 (Unix seconds, UTC) | yes (range) | Time-window filtering. |
| `event_count` | int32 | no | Display only; not used for filtering in V1. |
| `severity_max` | string | **yes** | Client-side severity filter ("only show me err+ matches"). |
| `resolution_present` | bool | **yes** | Critical for `suggest_fix` — filter to incidents with known fixes. |
| `source_type` | string (`"incident"`) | yes | Future-proofs against multi-source collections; today this collection is incident-only but indexed defensively. |
| `schema_version` | int32 | yes | Allows mixed-version coexistence during a backfill window. |

**Why timestamps as int64 not strings:** Qdrant range filters require numeric types. The SQLite source-of-truth keeps RFC 3339; the payload mirror keeps Unix seconds. Renderer must convert at write time.

---

## 6. Collection identity

- **Collection name:** `incidents` — **fixed**. Configurable in code only (a `const COLLECTION_NAME: &str` in `src/app/rag.rs`), not via runtime config. Per resolved §13: axon does not expose payload-filter passthrough, so we use a dedicated collection rather than filtering by `source_type` inside axon's default collection.
- **Vector dimension:** inherited from axon's TEI backend. Documented dynamically — the renderer never hardcodes a dimension. If axon swaps TEI models, the entire collection re-embeds (handled axon-side; from our perspective, we re-run the 90-day backfill).
- **Distance metric:** **cosine** — axon's default for TEI-backed collections. Verify with `axon doctor` on first deployment; if a future axon change defaults to a different metric we accept that change (the embedding model controls what's meaningful, not us).
- **Indexed payload fields (must be set in collection schema for performant client-side filtering):**
  - `host`
  - `app_name`
  - `signature_hash`
  - `signature_hash_xhost`
  - `resolution_present`
  - `severity_max`

  These are the fields the read path filters/sorts on after retrieval (spec §7). Indexing them in Qdrant turns post-retrieval filtering from O(N hits) Rust-side filtering into O(log N) Qdrant-side, which matters once the corpus grows.

  Non-indexed payload fields (`incident_id`, `source`, `first_seen_ts`, `last_seen_ts`, `event_count`, `source_type`, `schema_version`) are still queryable, just slower; that's acceptable for their usage patterns.

**Initial provisioning:** the cortex startup checks for the collection's existence and creates it with the indexed-field set above if missing. This is the only piece of axon coordination required from us — the spec marks it as "coordinate with axon owner."

---

## 7. Compatibility & schema bumps

- **V1:** `schema_version: 1`. This document defines V1 in its entirety.
- **Bump policy.** Any change to the card template (§2), placeholder set (§2), or payload field set (§5) requires a `schema_version` increment. Minor allowlist additions for an existing source do **not** bump the version — they trigger a per-source backfill (§4) but the wire format is unchanged.
- **Backfill on bump.** When `schema_version` bumps, all incidents in the **freshness window (last 90 days)** are re-rendered and re-embedded. Older incidents are left at their previous schema version — recall@5 against ancient incidents is already noisy, and re-embedding a year of history burns axon/TEI capacity for diminishing returns.
- **Mixed-version reads.** During a backfill, `incidents` will hold a mix of `schema_version: 1` and `schema_version: N` points. Retrieval treats them identically — the embedded text is what matters for similarity, and the payload fields documented here are stable across V1 (additions only, no removals or renames). If a future bump removes or renames a payload field, the read path must gate on `schema_version` to stay correct.

---

## Self-check

- Templates render with realistic UUIDv7-shaped IDs, plausible blake3-hex signature hashes, RFC 3339 timestamps with second precision, and sorted structured-field keys.
- Sample lines are believable per source: OOM-killer messages match the real kernel format; docker events match the dockerd event log shape; fail2ban lines match upstream output (with the `/24` redaction applied as the spec mandates).
- Payload schema field set matches what `similar_incidents` needs for client-side filtering per spec §7: host/app filters, `signature_hash_xhost` for cross-host, `resolution_present` for `suggest_fix`, `severity_max` for severity filtering, timestamps for recency boost.
