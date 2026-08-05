# Canonical Entity Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the clean hard-break canonical entity-resolution layer for Cortex so `plex` resolves to `logical_service:plex`, concrete instances such as `service_instance:nashost/plex`, and evidence-backed graph/query neighborhoods without preserving old nested service keys.

**Architecture:** Add a small neutral resolver module under `src/db/entity_resolution.rs` and `src/db/entity_resolution/*` that owns key grammar, source observations, deterministic resolver decisions, diagnostics, and stale-key classification. Runtime graph projection, inventory projection, map/findings, and `topic_correlate` consume resolver decisions instead of building service topology strings inline. The graph remains a rebuildable SQLite projection over authoritative logs, heartbeats, inventory, sessions, signatures, and agent Docker metadata.

**Tech Stack:** Rust 1.86 / edition 2024, rusqlite + SQLite/FTS5, serde/serde_json, existing Cortex service layer, existing MCP/CLI schema generation, Beads/Dolt for task tracking.

## Global Constraints

- This is a clean hard break in the graph contract. Do not preserve support for previous graph key shapes as public API behavior.
- Do not keep `nashost:plex`, `nashost:plex:plex`, or `plex/plex/plex` as accepted service identity inputs.
- Do not add a transitional lookup layer for previous key shapes.
- Do not emit previous `service` topology rows as part of the new canonical graph projection.
- Historical rows can exist in an unrebuilt database before migration/rebuild, but the implementation target is replacement, not coexistence.
- Any tests that mention previous key shapes must assert removal/rejection, not continued operation.
- Keep logical identity and deployment topology separate. `plex` is the logical service identity; `nashost/plex` is a host-scoped runtime instance.
- Do not silently redefine `app`. It remains a raw observed log/error application label.
- Agent-first Docker ingest is the supported Docker identity source for this milestone. Central Docker pull is not proof for the new behavior.
- Verified inventory/heartbeat/agent metadata outranks sender-claimed syslog hostnames and weak log app labels.
- Ambiguous resolution must be represented as candidates/open questions, not silently guessed.
- Resolver matching is deterministic first. No LLM or fuzzy substring matching in this milestone.
- Redaction must cover graph identifiers as well as evidence excerpts: canonical keys, display labels, aliases, source IDs, relationship keys, diagnostics, and safe display fields.
- Observations must stay chunk-local or aggregated unless persistence is justified by implementation evidence. Do not add a per-log resolver-observation table in the first milestone.
- Target resolver complexity is `O(log_rows + unique_observations)`, not `O(log_rows * resolver_lookups)`.
- `topic_correlate plex` must use service-instance predicates and inclusion reasons. It must not silently expand to all logs for the host running Plex.
- Production proof starts read-only with old-key counts, resolver diagnostics, query plans, and bounded timings. Any live rebuild requires WAL-safe backup, off-peak execution, timeout, rollback instructions, and post-rebuild zero-count assertions.
- Follow repo style: Rust sidecar tests live beside source modules as `*_tests.rs`; source files keep only the `#[cfg(test)] #[path = "..._tests.rs"] mod tests;` hook.
- Use Beads for tracking: claim the child bead before implementing its task and close it only after the validation commands pass.

---

## File Structure

- Create `src/db/entity_resolution.rs`: public module entry for resolver vocabulary, observations, decisions, and adapters. It re-exports only the small API used by projection/query code.
- Create `src/db/entity_resolution/vocab.rs`: canonical entity/relationship/reason constants, key builders, stale-key classifiers, graph projection contract constants.
- Create `src/db/entity_resolution/observation.rs`: bounded typed observations, trust/source metadata, safe display values, agent-Docker observation structs.
- Create `src/db/entity_resolution/resolver.rs`: deterministic resolution rules, ranked evidence, ambiguity records, diagnostics, and candidate caps.
- Create `src/db/entity_resolution/adapters.rs`: pure adapters that convert inventory services, agent Docker rows, raw app labels, domains/routes, storage artifacts, and AI/operator rows into observations.
- Create `src/db/entity_resolution_tests.rs`: table-driven unit tests for key grammar, observations, resolver decisions, diagnostics, and legacy-shape rejection.
- Modify `src/db.rs`: add `pub mod entity_resolution;` and re-export the stable resolver API needed outside `db`.
- Modify `src/db/graph.rs`: add new vocabulary constants or import them from `entity_resolution`; update CHECK-driven staging/projection behavior, graph walks, runtime graph projection, stale projection contract handling, and service-instance projection.
- Modify `src/db/pool.rs`: add schema migration for new graph vocabulary, projection contract metadata, cleanup/rebuild behavior, and any needed indexes.
- Modify `src/db/pool_tests.rs`: migration tests for populated DBs containing old service rows and bad nested app rows.
- Modify `src/db/graph_tests.rs`: runtime graph projection tests for logical-service/service-instance output and old-key absence.
- Modify `src/db/graph_inventory.rs`: convert inventory services, compose projects, routes, storage, and networks through resolver decisions.
- Modify `src/db/graph_inventory/sql.rs`: remove old `service_key(host:name)` canonical topology helper; keep only generic safe inventory key helpers.
- Modify `src/db/graph_inventory_tests.rs`: inventory projection tests for service instances, compose/storage/route edges, and old-key absence.
- Modify `src/agent/docker.rs`: produce structured agent-attested Docker identity metadata alongside the current syslog line.
- Modify `src/agent/docker_tests.rs`: metadata and long app-name fallback tests.
- Modify `src/agent/syslog_sender.rs` only if the chosen structured envelope needs a helper for safe metadata encoding.
- Modify `docs/contracts/log-row-shape.md`, `docs/contracts/metadata-json-shape.md`, and `docs/contracts/source-kinds.md`: document the supported agent-first Docker identity shape and central Docker non-proof status.
- Modify `src/db/queries.rs`: replace service/container host-splitting fan-out with resolver-backed service-instance predicates, bounded graph walks, and inclusion metadata.
- Modify `src/db/queries_graph_tests.rs`: graph walk caps, service-instance log predicate, and no-host-wide-fan-out tests.
- Modify `src/app/models/ai_incidents.rs`: add `resolver_status`, `fallback_kind`, and `inclusion_reason` fields to topic responses.
- Modify `src/app/services/topic_correlate.rs`: resolve topics through the resolver and annotate timeline entries with inclusion metadata.
- Modify `src/app/services/topic_correlate_tests.rs`: Plex query tests, old-key rejection tests, and degraded host-fallback tests.
- Modify `src/app/services/graph.rs`: reject old key shapes before graph lookup and expose resolver diagnostics/candidates.
- Modify `src/app/services/graph_support.rs` and `src/app/services/graph_safety.rs`: shared sanitization for identifiers and diagnostics.
- Modify `src/app/models/graph.rs`: add resolver diagnostic response fields where existing graph responses need them.
- Modify `src/app/services/map_answers.rs`: route `service_dependencies` through `service_instance` lookup rather than `host:service`.
- Modify `src/app/services/map_findings/risky_mounts.rs`: stop synthesizing old `host:service` keys.
- Modify `src/app/services/map_tests.rs` and `src/app/services/map_findings/risky_mounts_tests.rs`: old service key rejection and new service-instance target tests.
- Modify `src/mcp/schemas.rs` and `src/mcp/schemas_tests.rs`: expose `logical_service` / `service_instance`, remove supported `service` identity, document rejection metadata.
- Modify `src/cli/output/graph.rs` and `src/cli/output/graph_tests.rs`: display new entity types and resolver diagnostics.
- Modify `docs/contracts/investigation-graph.md`, `docs/contracts/current-schema.sql`, `openwiki/inventory-graph.md`, `docs/mcp/TOOLS.md`, `docs/mcp/SCHEMA.md`, `docs/CLI.md`, and `README.md`: document new graph contract and Plex proof.
- Create `scripts/validate-canonical-plex-graph.sh`: fixture/production-safe proof workflow that defaults to read-only checks and refuses live rebuild without explicit operator action.
- Modify `scripts/check-public-identity.sh` only if documentation introduces stale public tokens that should be scanned.

## Shared Interfaces

All tasks use these names. If implementation discovers an unavoidable naming conflict, update this plan and every affected task before coding.

```rust
// src/db/entity_resolution/vocab.rs
pub const ENTITY_TYPE_LOGICAL_SERVICE: &str = "logical_service";
pub const ENTITY_TYPE_SERVICE_INSTANCE: &str = "service_instance";
pub const REL_INSTANCE_OF: &str = "instance_of";
pub const REASON_RESOLVER_INSTANCE_OF: &str = "resolver_instance_of";
pub const REASON_RESOLVER_SERVICE_INSTANCE: &str = "resolver_service_instance";
pub const REASON_RESOLVER_RAW_APP_LABEL: &str = "resolver_raw_app_label";
pub const GRAPH_PROJECTION_CONTRACT_KEY: &str = "graph_projection_contract";
pub const GRAPH_PROJECTION_CONTRACT_V2: &str = "entity_resolution_v2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyShape {
    HostService,
    HostProjectService,
    SlashTriplet,
}

pub fn logical_service_key(name: &str) -> Option<String>;
pub fn service_instance_key(host: &str, service: &str) -> Option<String>;
pub fn split_service_instance_key(key: &str) -> Option<(&str, &str)>;
pub fn classify_legacy_shape(value: &str) -> Option<LegacyShape>;
```

```rust
// src/db/entity_resolution/observation.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolverTrust {
    Verified,
    Claimed,
    Inferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationKind {
    Host,
    LogicalService,
    ServiceInstance,
    Container,
    ComposeProject,
    Domain,
    ReverseProxy,
    Storage,
    ConfigArtifact,
    RawAppLabel,
    AiProject,
    AiSession,
    Command,
    User,
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverObservation {
    pub kind: ObservationKind,
    pub observed_key: String,
    pub display_label: String,
    pub host_key: Option<String>,
    pub logical_service_key: Option<String>,
    pub service_instance_key: Option<String>,
    pub source_kind: String,
    pub source_id: String,
    pub evidence_path: String,
    pub observed_at: String,
    pub trust: ResolverTrust,
    pub structured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDockerIdentity {
    pub agent_host: String,
    pub container_id: String,
    pub container_name: String,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    pub image: Option<String>,
    pub stream: String,
    pub observed_at: String,
}
```

```rust
// src/db/entity_resolution/resolver.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverStatus {
    Resolved,
    Ambiguous,
    RejectedLegacyShape,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverEvidence {
    pub rule_id: &'static str,
    pub source_kind: String,
    pub source_id: String,
    pub evidence_path: String,
    pub trust: ResolverTrust,
    pub safe_excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntityDecision {
    pub entity_type: &'static str,
    pub canonical_key: String,
    pub display_label: String,
    pub status: ResolverStatus,
    pub trust: ResolverTrust,
    pub evidence: Vec<ResolverEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverDiagnostic {
    pub status: ResolverStatus,
    pub input: String,
    pub reason: String,
    pub candidates: Vec<ResolvedEntityDecision>,
    pub evidence_sample: Vec<ResolverEvidence>,
    pub total_evidence_count: usize,
}

pub fn resolve_observations(observations: &[ResolverObservation]) -> Vec<ResolvedEntityDecision>;
pub fn diagnose_lookup_input(input: &str) -> ResolverDiagnostic;
```

---

### Task 1: Vocabulary, Schema Contract, and Hard-Break Cutover

**Files:**
- Create: `src/db/entity_resolution.rs`
- Create: `src/db/entity_resolution/vocab.rs`
- Create: `src/db/entity_resolution_tests.rs`
- Modify: `src/db.rs`
- Modify: `src/db/graph.rs`
- Modify: `src/db/pool.rs`
- Modify: `src/db/pool_tests.rs`
- Modify: `src/mcp/schemas.rs`
- Modify: `src/mcp/schemas_tests.rs`
- Modify: `src/cli/output/graph.rs`
- Modify: `src/cli/output/graph_tests.rs`
- Modify: `docs/contracts/investigation-graph.md`
- Modify: `docs/contracts/current-schema.sql`
- Modify: `openwiki/inventory-graph.md`

**Interfaces:**
- Consumes: no new task-local interfaces.
- Produces: `ENTITY_TYPE_LOGICAL_SERVICE`, `ENTITY_TYPE_SERVICE_INSTANCE`, `REL_INSTANCE_OF`, `GRAPH_PROJECTION_CONTRACT_V2`, `logical_service_key`, `service_instance_key`, `split_service_instance_key`, `classify_legacy_shape`, and public graph schema acceptance for `logical_service`, `service_instance`, `instance_of`.

- [ ] **Step 1: Claim the Bead**

Run:

```bash
bd update syslog-mcp-vkln9.1 --claim
```

Expected: command exits `0` and marks `syslog-mcp-vkln9.1` in progress.

- [ ] **Step 2: Write failing vocabulary tests**

Create `src/db/entity_resolution.rs`:

```rust
pub mod vocab;

#[cfg(test)]
#[path = "entity_resolution_tests.rs"]
mod tests;
```

Create `src/db/entity_resolution/vocab.rs` with only the constants and signatures from Shared Interfaces. Return `None` from the functions for now so the behavior tests fail.

Create `src/db/entity_resolution_tests.rs`:

```rust
use super::vocab::*;

#[test]
fn canonical_service_keys_separate_logic_from_topology() {
    assert_eq!(logical_service_key(" Plex "), Some("plex".to_string()));
    assert_eq!(
        service_instance_key("Nashost", " Plex "),
        Some("nashost/plex".to_string())
    );
    assert_eq!(split_service_instance_key("nashost/plex"), Some(("nashost", "plex")));
}

#[test]
fn old_nested_service_shapes_are_classified_not_normalized() {
    assert_eq!(classify_legacy_shape("nashost:plex"), Some(LegacyShape::HostService));
    assert_eq!(
        classify_legacy_shape("nashost:plex:plex"),
        Some(LegacyShape::HostProjectService)
    );
    assert_eq!(
        classify_legacy_shape("plex/plex/plex"),
        Some(LegacyShape::SlashTriplet)
    );
    assert_eq!(classify_legacy_shape("plex"), None);
    assert_eq!(classify_legacy_shape("nashost/plex"), None);
}
```

Modify `src/db.rs`:

```rust
pub mod entity_resolution;
```

- [ ] **Step 3: Run the vocabulary tests and verify failure**

Run:

```bash
cargo test db::entity_resolution::tests:: -- --nocapture
```

Expected: FAIL because the key functions return `None` or do not classify old shapes.

- [ ] **Step 4: Implement the key grammar**

Replace `src/db/entity_resolution/vocab.rs` with:

```rust
pub const ENTITY_TYPE_LOGICAL_SERVICE: &str = "logical_service";
pub const ENTITY_TYPE_SERVICE_INSTANCE: &str = "service_instance";
pub const REL_INSTANCE_OF: &str = "instance_of";
pub const REASON_RESOLVER_INSTANCE_OF: &str = "resolver_instance_of";
pub const REASON_RESOLVER_SERVICE_INSTANCE: &str = "resolver_service_instance";
pub const REASON_RESOLVER_RAW_APP_LABEL: &str = "resolver_raw_app_label";
pub const GRAPH_PROJECTION_CONTRACT_KEY: &str = "graph_projection_contract";
pub const GRAPH_PROJECTION_CONTRACT_V2: &str = "entity_resolution_v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyShape {
    HostService,
    HostProjectService,
    SlashTriplet,
}

pub fn logical_service_key(name: &str) -> Option<String> {
    canonical_component(name)
}

pub fn service_instance_key(host: &str, service: &str) -> Option<String> {
    Some(format!(
        "{}/{}",
        canonical_component(host)?,
        canonical_component(service)?
    ))
}

pub fn split_service_instance_key(key: &str) -> Option<(&str, &str)> {
    let (host, service) = key.split_once('/')?;
    if host.is_empty() || service.is_empty() || service.contains('/') {
        return None;
    }
    Some((host, service))
}

pub fn classify_legacy_shape(value: &str) -> Option<LegacyShape> {
    let trimmed = value.trim();
    let colon_count = trimmed.matches(':').count();
    if colon_count == 1 {
        return Some(LegacyShape::HostService);
    }
    if colon_count >= 2 {
        return Some(LegacyShape::HostProjectService);
    }
    let slash_count = trimmed.matches('/').count();
    if slash_count >= 2 {
        return Some(LegacyShape::SlashTriplet);
    }
    None
}

fn canonical_component(value: &str) -> Option<String> {
    let out = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    (!out.is_empty()).then_some(out)
}
```

- [ ] **Step 5: Widen graph vocabulary and schema docs**

Modify `src/db/graph.rs`:

```rust
pub const ENTITY_TYPE_LOGICAL_SERVICE: &str =
    crate::db::entity_resolution::vocab::ENTITY_TYPE_LOGICAL_SERVICE;
pub const ENTITY_TYPE_SERVICE_INSTANCE: &str =
    crate::db::entity_resolution::vocab::ENTITY_TYPE_SERVICE_INSTANCE;
pub const REL_INSTANCE_OF: &str = crate::db::entity_resolution::vocab::REL_INSTANCE_OF;
pub const REASON_RESOLVER_INSTANCE_OF: &str =
    crate::db::entity_resolution::vocab::REASON_RESOLVER_INSTANCE_OF;
pub const REASON_RESOLVER_SERVICE_INSTANCE: &str =
    crate::db::entity_resolution::vocab::REASON_RESOLVER_SERVICE_INSTANCE;
pub const REASON_RESOLVER_RAW_APP_LABEL: &str =
    crate::db::entity_resolution::vocab::REASON_RESOLVER_RAW_APP_LABEL;
```

Update `ENTITY_TYPES`, `RELATIONSHIP_TYPES`, and `REASON_CODES` to include:

```rust
ENTITY_TYPE_LOGICAL_SERVICE,
ENTITY_TYPE_SERVICE_INSTANCE,
REL_INSTANCE_OF,
REASON_RESOLVER_INSTANCE_OF,
REASON_RESOLVER_SERVICE_INSTANCE,
REASON_RESOLVER_RAW_APP_LABEL,
```

Keep `ENTITY_TYPE_SERVICE` available during this task so existing tests compile; later projection/query tasks remove supported service identity behavior from outputs.

Modify `src/db/pool.rs` graph CHECK constraints and `docs/contracts/current-schema.sql` to include:

```sql
'logical_service', 'service_instance'
```

in `graph_entities.entity_type`, and:

```sql
'instance_of'
```

in `graph_relationships.relationship_type`, and:

```sql
'resolver_instance_of', 'resolver_service_instance', 'resolver_raw_app_label'
```

in `graph_relationships.reason_code` and `graph_relationship_evidence.reason_code`.

- [ ] **Step 6: Add projection contract metadata tests**

Add to `src/db/pool_tests.rs`:

```rust
#[test]
fn graph_schema_accepts_entity_resolution_vocabulary() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("resolver-vocab.db"))).unwrap();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES
            ('logical_service', 'plex', 'plex', 'resolver', 'fixture', 'verified'),
            ('service_instance', 'nashost/plex', 'nashost/plex', 'resolver', 'fixture', 'verified')",
        [],
    )
    .unwrap();
    let service = conn.query_row(
        "SELECT id FROM graph_entities WHERE entity_type = 'logical_service'",
        [],
        |row| row.get::<_, i64>(0),
    ).unwrap();
    let instance = conn.query_row(
        "SELECT id FROM graph_entities WHERE entity_type = 'service_instance'",
        [],
        |row| row.get::<_, i64>(0),
    ).unwrap();
    conn.execute(
        "INSERT INTO graph_relationships
            (relationship_key, src_entity_id, dst_entity_id, relationship_type,
             reason_code, trust_level, confidence)
         VALUES (?1, ?2, ?3, 'instance_of', 'resolver_instance_of', 'verified', 1.0)",
        rusqlite::params![format!("{instance}:instance_of:{service}"), instance, service],
    )
    .unwrap();
}
```

Add a second test that inserts old rows, runs the new cleanup helper from Step 7, and asserts they are gone:

```rust
#[test]
fn stale_service_topology_cleanup_removes_old_canonical_rows() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("stale-service-cleanup.db"))).unwrap();
    let mut conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES
            ('service', 'nashost:plex', 'plex', 'log', 'fixture', 'inferred'),
            ('service', 'nashost:plex:plex', 'nashost/plex/plex', 'log', 'fixture', 'inferred'),
            ('app', 'plex/plex/plex', 'plex/plex/plex', 'log', 'fixture', 'claimed')",
        [],
    ).unwrap();
    crate::db::graph::cleanup_legacy_service_topology(&mut conn).unwrap();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_entities
          WHERE (entity_type = 'service' AND canonical_key LIKE '%:%')
             OR (entity_type = 'app' AND canonical_key = 'plex/plex/plex')",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 0);
}
```

- [ ] **Step 7: Implement stale-row cleanup helper**

Add to `src/db/graph.rs`:

```rust
pub fn cleanup_legacy_service_topology(conn: &mut rusqlite::Connection) -> Result<()> {
    let _guard = write_lock();
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM graph_relationship_evidence
          WHERE relationship_id IN (
              SELECT r.id
                FROM graph_relationships r
                JOIN graph_entities src ON src.id = r.src_entity_id
                JOIN graph_entities dst ON dst.id = r.dst_entity_id
               WHERE src.entity_type = 'service'
                  OR dst.entity_type = 'service'
                  OR (src.entity_type = 'app' AND src.canonical_key LIKE '%/%/%')
                  OR (dst.entity_type = 'app' AND dst.canonical_key LIKE '%/%/%')
          )",
        [],
    )?;
    tx.execute(
        "DELETE FROM graph_relationships
          WHERE src_entity_id IN (
              SELECT id FROM graph_entities
               WHERE entity_type = 'service'
                  OR (entity_type = 'app' AND canonical_key LIKE '%/%/%')
          )
             OR dst_entity_id IN (
              SELECT id FROM graph_entities
               WHERE entity_type = 'service'
                  OR (entity_type = 'app' AND canonical_key LIKE '%/%/%')
          )",
        [],
    )?;
    tx.execute(
        "DELETE FROM graph_entity_aliases
          WHERE entity_id IN (
              SELECT id FROM graph_entities
               WHERE entity_type = 'service'
                  OR (entity_type = 'app' AND canonical_key LIKE '%/%/%')
          )",
        [],
    )?;
    tx.execute(
        "DELETE FROM graph_entities
          WHERE entity_type = 'service'
             OR (entity_type = 'app' AND canonical_key LIKE '%/%/%')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}
```

- [ ] **Step 8: Update public schema enum tests**

In `src/mcp/schemas_tests.rs`, add assertions that `logical_service` and `service_instance` are present in graph entity enums and old `service` is not described as service identity. Use exact JSON lookups already used by nearby schema tests.

Add a CLI output test in `src/cli/output/graph_tests.rs` that formats `logical_service` and `service_instance` rows without falling back to generic unknown-type output.

- [ ] **Step 9: Run Task 1 tests**

Run:

```bash
cargo test db::entity_resolution::tests:: -- --nocapture
cargo test db::pool_tests::graph_schema_accepts_entity_resolution_vocabulary -- --nocapture
cargo test db::pool_tests::stale_service_topology_cleanup_removes_old_canonical_rows -- --nocapture
cargo test mcp::schemas_tests:: -- --nocapture
cargo test cli::output::graph_tests:: -- --nocapture
```

Expected: all PASS.

- [ ] **Step 10: Commit Task 1**

```bash
git add src/db.rs src/db/entity_resolution.rs src/db/entity_resolution/vocab.rs src/db/entity_resolution_tests.rs src/db/graph.rs src/db/pool.rs src/db/pool_tests.rs src/mcp/schemas.rs src/mcp/schemas_tests.rs src/cli/output/graph.rs src/cli/output/graph_tests.rs docs/contracts/investigation-graph.md docs/contracts/current-schema.sql openwiki/inventory-graph.md
git commit -m "feat: define canonical service graph vocabulary"
```

---

### Task 2: Bounded Observation Extraction Model

**Files:**
- Create: `src/db/entity_resolution/observation.rs`
- Create: `src/db/entity_resolution/adapters.rs`
- Modify: `src/db/entity_resolution.rs`
- Modify: `src/db/entity_resolution_tests.rs`
- Modify: `docs/contracts/investigation-graph.md`

**Interfaces:**
- Consumes: vocabulary functions from Task 1.
- Produces: `ResolverObservation`, `ResolverTrust`, `ObservationKind`, `AgentDockerIdentity`, `observations_from_agent_docker_identity`, `observations_from_raw_app_label`, `observations_from_inventory_service`, and safe display helpers.

- [ ] **Step 1: Claim the Bead**

Run:

```bash
bd update syslog-mcp-vkln9.2 --claim
```

Expected: command exits `0`.

- [ ] **Step 2: Write failing observation tests**

Append to `src/db/entity_resolution_tests.rs`:

```rust
use super::adapters::*;
use super::observation::*;

#[test]
fn agent_docker_identity_extracts_structured_service_instance() {
    let identity = AgentDockerIdentity {
        agent_host: "Nashost".to_string(),
        container_id: "abcdef1234567890".to_string(),
        container_name: "plex".to_string(),
        compose_project: Some("plex".to_string()),
        compose_service: Some("plex".to_string()),
        image: Some("lscr.io/linuxserver/plex:latest".to_string()),
        stream: "stdout".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let observations = observations_from_agent_docker_identity(&identity);
    assert!(observations.iter().any(|o| {
        o.kind == ObservationKind::ServiceInstance
            && o.service_instance_key.as_deref() == Some("nashost/plex")
            && o.logical_service_key.as_deref() == Some("plex")
            && o.trust == ResolverTrust::Verified
            && o.structured
    }));
}

#[test]
fn raw_app_label_does_not_create_logical_service_observation_by_itself() {
    let observations = observations_from_raw_app_label(
        "plex/plex/plex",
        "nashost",
        "log",
        "42",
        "2026-01-01T00:00:00Z",
    );
    assert!(observations.iter().any(|o| o.kind == ObservationKind::RawAppLabel));
    assert!(!observations.iter().any(|o| o.kind == ObservationKind::LogicalService));
}

#[test]
fn safe_observation_display_redacts_sensitive_values() {
    assert_eq!(safe_display_value("https://user:pass@example.test/path"), "[redacted]");
    assert_eq!(safe_display_value("/home/jmagar/.cortex/token.txt"), "[redacted]");
    assert_eq!(safe_display_value("plex"), "plex");
}
```

- [ ] **Step 3: Run observation tests and verify failure**

Run:

```bash
cargo test db::entity_resolution::tests:: -- --nocapture
```

Expected: FAIL because `observation` and `adapters` modules do not exist.

- [ ] **Step 4: Implement observation structs**

Create `src/db/entity_resolution/observation.rs` using the Shared Interfaces. Add this helper:

```rust
pub fn safe_display_value(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let sensitive = lower.contains("://") && lower.contains('@')
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("secret")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("/home/")
        || lower.contains("/users/")
        || lower.contains("metadata_json")
        || lower.contains("cache_path")
        || lower.contains("source_path");
    if sensitive {
        return "[redacted]".to_string();
    }
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(128)
        .collect()
}
```

- [ ] **Step 5: Implement source adapters**

Create `src/db/entity_resolution/adapters.rs`:

```rust
use super::observation::*;
use super::vocab::{logical_service_key, service_instance_key};

pub fn observations_from_agent_docker_identity(
    identity: &AgentDockerIdentity,
) -> Vec<ResolverObservation> {
    let Some(host_key) = logical_service_key(&identity.agent_host) else {
        return Vec::new();
    };
    let service_name = identity
        .compose_service
        .as_deref()
        .unwrap_or(identity.container_name.as_str());
    let Some(logical_key) = logical_service_key(service_name) else {
        return Vec::new();
    };
    let Some(instance_key) = service_instance_key(&host_key, &logical_key) else {
        return Vec::new();
    };
    vec![
        ResolverObservation {
            kind: ObservationKind::Host,
            observed_key: host_key.clone(),
            display_label: safe_display_value(&identity.agent_host),
            host_key: Some(host_key.clone()),
            logical_service_key: None,
            service_instance_key: None,
            source_kind: "agent-docker".to_string(),
            source_id: identity.container_id.clone(),
            evidence_path: "agent_docker.host".to_string(),
            observed_at: identity.observed_at.clone(),
            trust: ResolverTrust::Verified,
            structured: true,
        },
        ResolverObservation {
            kind: ObservationKind::LogicalService,
            observed_key: logical_key.clone(),
            display_label: safe_display_value(service_name),
            host_key: None,
            logical_service_key: Some(logical_key.clone()),
            service_instance_key: None,
            source_kind: "agent-docker".to_string(),
            source_id: identity.container_id.clone(),
            evidence_path: "agent_docker.compose_service".to_string(),
            observed_at: identity.observed_at.clone(),
            trust: ResolverTrust::Verified,
            structured: true,
        },
        ResolverObservation {
            kind: ObservationKind::ServiceInstance,
            observed_key: instance_key.clone(),
            display_label: instance_key.clone(),
            host_key: Some(host_key),
            logical_service_key: Some(logical_key),
            service_instance_key: Some(instance_key),
            source_kind: "agent-docker".to_string(),
            source_id: identity.container_id.clone(),
            evidence_path: "agent_docker.compose_project_service".to_string(),
            observed_at: identity.observed_at.clone(),
            trust: ResolverTrust::Verified,
            structured: true,
        },
    ]
}

pub fn observations_from_raw_app_label(
    app_name: &str,
    host: &str,
    source_kind: &str,
    source_id: &str,
    observed_at: &str,
) -> Vec<ResolverObservation> {
    let observed_key = app_name.trim().to_ascii_lowercase();
    vec![ResolverObservation {
        kind: ObservationKind::RawAppLabel,
        observed_key,
        display_label: safe_display_value(app_name),
        host_key: super::vocab::logical_service_key(host),
        logical_service_key: None,
        service_instance_key: None,
        source_kind: source_kind.to_string(),
        source_id: source_id.to_string(),
        evidence_path: "logs.app_name".to_string(),
        observed_at: observed_at.to_string(),
        trust: ResolverTrust::Claimed,
        structured: false,
    }]
}
```

Add `observations_from_inventory_service` after inspecting `crate::inventory::schema::InventoryService`. It must set `trust = ResolverTrust::Verified` for observed/verified inventory and include service instance, logical service, host, mounts, domains, and config artifacts where the inventory object has those fields.

- [ ] **Step 6: Export modules**

Modify `src/db/entity_resolution.rs`:

```rust
pub mod adapters;
pub mod observation;
pub mod vocab;

pub use adapters::*;
pub use observation::*;
pub use vocab::*;

#[cfg(test)]
#[path = "entity_resolution_tests.rs"]
mod tests;
```

- [ ] **Step 7: Run Task 2 tests**

Run:

```bash
cargo test db::entity_resolution::tests:: -- --nocapture
```

Expected: all Task 1 and Task 2 entity-resolution tests PASS.

- [ ] **Step 8: Commit Task 2**

```bash
git add src/db/entity_resolution.rs src/db/entity_resolution/observation.rs src/db/entity_resolution/adapters.rs src/db/entity_resolution_tests.rs docs/contracts/investigation-graph.md
git commit -m "feat: add bounded resolver observations"
```

---

### Task 3: Deterministic Resolver Rules and Diagnostics

**Files:**
- Create: `src/db/entity_resolution/resolver.rs`
- Modify: `src/db/entity_resolution.rs`
- Modify: `src/db/entity_resolution_tests.rs`
- Modify: `src/app/models/graph.rs`
- Modify: `src/app/services/graph.rs`

**Interfaces:**
- Consumes: `ResolverObservation`, `ResolverTrust`, key grammar from Tasks 1-2.
- Produces: `ResolverStatus`, `ResolverEvidence`, `ResolvedEntityDecision`, `ResolverDiagnostic`, `resolve_observations`, `diagnose_lookup_input`.

- [ ] **Step 1: Claim the Bead**

Run:

```bash
bd update syslog-mcp-vkln9.3 --claim
```

Expected: command exits `0`.

- [ ] **Step 2: Write failing resolver tests**

Append to `src/db/entity_resolution_tests.rs`:

```rust
use super::resolver::*;

#[test]
fn resolver_converges_duplicate_hosts_under_one_logical_service() {
    let nashost = ResolverObservation {
        kind: ObservationKind::ServiceInstance,
        observed_key: "nashost/plex".to_string(),
        display_label: "nashost/plex".to_string(),
        host_key: Some("nashost".to_string()),
        logical_service_key: Some("plex".to_string()),
        service_instance_key: Some("nashost/plex".to_string()),
        source_kind: "app_inventory".to_string(),
        source_id: "inventory:nashost".to_string(),
        evidence_path: "inventory.services.plex".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
        trust: ResolverTrust::Verified,
        structured: true,
    };
    let backuphost = ResolverObservation {
        service_instance_key: Some("backuphost/plex".to_string()),
        host_key: Some("backuphost".to_string()),
        source_id: "inventory:backuphost".to_string(),
        observed_key: "backuphost/plex".to_string(),
        display_label: "backuphost/plex".to_string(),
        ..nashost.clone()
    };
    let decisions = resolve_observations(&[nashost, backuphost]);
    assert!(decisions.iter().any(|d| {
        d.entity_type == ENTITY_TYPE_LOGICAL_SERVICE && d.canonical_key == "plex"
    }));
    assert!(decisions.iter().any(|d| {
        d.entity_type == ENTITY_TYPE_SERVICE_INSTANCE && d.canonical_key == "nashost/plex"
    }));
    assert!(decisions.iter().any(|d| {
        d.entity_type == ENTITY_TYPE_SERVICE_INSTANCE && d.canonical_key == "backuphost/plex"
    }));
}

#[test]
fn resolver_rejects_old_key_shapes_before_lookup() {
    for input in ["nashost:plex", "nashost:plex:plex", "plex/plex/plex"] {
        let diagnostic = diagnose_lookup_input(input);
        assert_eq!(diagnostic.status, ResolverStatus::RejectedLegacyShape);
        assert_eq!(diagnostic.reason, "rejected_legacy_shape");
        assert!(diagnostic.candidates.is_empty());
    }
}

#[test]
fn weak_raw_labels_do_not_upgrade_themselves() {
    let observations = observations_from_raw_app_label(
        "complex",
        "nashost",
        "log",
        "99",
        "2026-01-01T00:00:00Z",
    );
    let decisions = resolve_observations(&observations);
    assert!(!decisions.iter().any(|d| d.canonical_key == "plex"));
}
```

- [ ] **Step 3: Run resolver tests and verify failure**

Run:

```bash
cargo test db::entity_resolution::tests::resolver_ -- --nocapture
```

Expected: FAIL because the resolver module and types are missing.

- [ ] **Step 4: Implement resolver decisions**

Create `src/db/entity_resolution/resolver.rs` with the Shared Interfaces and this resolution behavior:

```rust
pub const MAX_RESOLVER_EVIDENCE_SAMPLE: usize = 5;
pub const MAX_RESOLVER_CANDIDATES: usize = 25;

pub fn resolve_observations(observations: &[ResolverObservation]) -> Vec<ResolvedEntityDecision> {
    let mut by_entity: std::collections::BTreeMap<(&'static str, String), Vec<ResolverEvidence>> =
        std::collections::BTreeMap::new();
    for obs in observations {
        match obs.kind {
            ObservationKind::LogicalService => {
                if let Some(key) = obs.logical_service_key.clone() {
                    by_entity.entry((ENTITY_TYPE_LOGICAL_SERVICE, key)).or_default().push(evidence(obs, "logical_service_observation"));
                }
            }
            ObservationKind::ServiceInstance => {
                if let Some(key) = obs.service_instance_key.clone() {
                    by_entity.entry((ENTITY_TYPE_SERVICE_INSTANCE, key)).or_default().push(evidence(obs, "service_instance_observation"));
                }
                if let Some(key) = obs.logical_service_key.clone() {
                    by_entity.entry((ENTITY_TYPE_LOGICAL_SERVICE, key)).or_default().push(evidence(obs, "service_instance_logical_service"));
                }
            }
            ObservationKind::RawAppLabel => {}
            _ => {}
        }
    }
    by_entity
        .into_iter()
        .map(|((entity_type, canonical_key), evidence)| {
            let trust = evidence.iter().map(|e| e.trust).min().unwrap_or(ResolverTrust::Inferred);
            ResolvedEntityDecision {
                entity_type,
                display_label: canonical_key.clone(),
                canonical_key,
                status: ResolverStatus::Resolved,
                trust,
                evidence: evidence.into_iter().take(MAX_RESOLVER_EVIDENCE_SAMPLE).collect(),
            }
        })
        .collect()
}

pub fn diagnose_lookup_input(input: &str) -> ResolverDiagnostic {
    if super::vocab::classify_legacy_shape(input).is_some() {
        return ResolverDiagnostic {
            status: ResolverStatus::RejectedLegacyShape,
            input: input.to_string(),
            reason: "rejected_legacy_shape".to_string(),
            candidates: Vec::new(),
            evidence_sample: Vec::new(),
            total_evidence_count: 0,
        };
    }
    ResolverDiagnostic {
        status: ResolverStatus::Degraded,
        input: input.to_string(),
        reason: "no_resolver_candidates".to_string(),
        candidates: Vec::new(),
        evidence_sample: Vec::new(),
        total_evidence_count: 0,
    }
}
```

Add helper `fn evidence(obs: &ResolverObservation, rule_id: &'static str) -> ResolverEvidence`.

- [ ] **Step 5: Wire graph lookup stale-key rejection**

In `src/app/services/graph.rs`, before `validate_graph_entity_type(&entity_type)?` in `resolve_graph_target_entity`, add:

```rust
if let Some(key) = target_key_for_legacy_check(&target) {
    let diagnostic = db::entity_resolution::diagnose_lookup_input(key);
    if diagnostic.status == db::entity_resolution::ResolverStatus::RejectedLegacyShape {
        return Err(ServiceError::InvalidInput(format!(
            "unsupported legacy graph service identity `{key}`: rejected_legacy_shape"
        )));
    }
}
```

Implement `target_key_for_legacy_check` in the same file:

```rust
fn target_key_for_legacy_check(target: &GraphTarget) -> Option<&str> {
    match target {
        GraphTarget::CanonicalKey { key, .. } => Some(key.as_str()),
        GraphTarget::Alias { alias_key, .. } => Some(alias_key.as_str()),
        GraphTarget::EntityId(_) => None,
    }
}
```

- [ ] **Step 6: Run Task 3 tests**

Run:

```bash
cargo test db::entity_resolution::tests::resolver_ -- --nocapture
cargo test app::services::graph -- --nocapture
```

Expected: all PASS; graph service tests that submit `nashost:plex` now expect `ServiceError::InvalidInput`.

- [ ] **Step 7: Commit Task 3**

```bash
git add src/db/entity_resolution.rs src/db/entity_resolution/resolver.rs src/db/entity_resolution_tests.rs src/app/models/graph.rs src/app/services/graph.rs
git commit -m "feat: add deterministic resolver diagnostics"
```

---

### Task 4: Structured Agent-First Docker Identity

**Files:**
- Modify: `src/agent/docker.rs`
- Modify: `src/agent/docker_tests.rs`
- Modify: `src/db/entity_resolution/adapters.rs`
- Modify: `src/db/entity_resolution_tests.rs`
- Modify: `docs/contracts/log-row-shape.md`
- Modify: `docs/contracts/metadata-json-shape.md`
- Modify: `docs/contracts/source-kinds.md`
- Modify: `openwiki/log-intelligence.md`

**Interfaces:**
- Consumes: `AgentDockerIdentity`, `observations_from_agent_docker_identity`.
- Produces: agent-forwarded Docker rows with structured identity metadata and tests proving long APP-NAME fallback does not lose canonical identity.

- [ ] **Step 1: Claim the Bead**

Run:

```bash
bd update syslog-mcp-vkln9.4 --claim
```

Expected: command exits `0`.

- [ ] **Step 2: Write failing agent Docker tests**

Append to `src/agent/docker_tests.rs`:

```rust
#[test]
fn container_identity_metadata_carries_compose_context() {
    let labels = HashMap::from([
        ("com.docker.compose.project".to_string(), "plex".to_string()),
        ("com.docker.compose.service".to_string(), "plex".to_string()),
        ("com.docker.compose.config-hash".to_string(), "abc".to_string()),
    ]);
    let metadata = container_identity_metadata(
        "nashost",
        "abcdef1234567890",
        "plex",
        "stdout",
        Some("lscr.io/linuxserver/plex:latest"),
        &labels,
    );
    assert_eq!(metadata["source_kind"], "agent-docker");
    assert_eq!(metadata["agent_docker"]["host"], "nashost");
    assert_eq!(metadata["agent_docker"]["container_id"], "abcdef1234567890");
    assert_eq!(metadata["agent_docker"]["compose_project"], "plex");
    assert_eq!(metadata["agent_docker"]["compose_service"], "plex");
}

#[test]
fn long_compose_app_name_still_has_structured_metadata() {
    let labels = HashMap::from([
        (
            "com.docker.compose.project".to_string(),
            "very-long-compose-project-name-for-plex-media-stack".to_string(),
        ),
        (
            "com.docker.compose.service".to_string(),
            "very-long-plex-service-name".to_string(),
        ),
    ]);
    let app_name = container_app_name("very-long-container-name-for-plex", &labels);
    assert!(app_name.len() > 48);
    let metadata = container_identity_metadata(
        "nashost",
        "abcdef1234567890",
        "very-long-container-name-for-plex",
        "stderr",
        None,
        &labels,
    );
    assert_eq!(
        metadata["agent_docker"]["compose_service"],
        "very-long-plex-service-name"
    );
}
```

- [ ] **Step 3: Run agent Docker tests and verify failure**

Run:

```bash
cargo test agent::docker_tests:: -- --nocapture
```

Expected: FAIL because `container_identity_metadata` does not exist.

- [ ] **Step 4: Implement structured metadata helper**

Modify `ContainerInfo` in `src/agent/docker.rs`:

```rust
struct ContainerInfo {
    id: String,
    name: String,
    app_name: String,
    image: Option<String>,
    labels: HashMap<String, String>,
}
```

In `list_containers`, carry `s.image` and cloned labels into `ContainerInfo`.

Add helper:

```rust
fn container_identity_metadata(
    host: &str,
    container_id: &str,
    container_name: &str,
    stream: &str,
    image: Option<&str>,
    labels: &HashMap<String, String>,
) -> serde_json::Value {
    serde_json::json!({
        "source_kind": "agent-docker",
        "agent_docker": {
            "host": host,
            "container_id": container_id,
            "container_name": container_name,
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
            "image": image,
            "stream": stream,
        }
    })
}
```

In `follow_container`, build metadata before formatting the line:

```rust
let stream = if is_stderr { "stderr" } else { "stdout" };
let metadata = container_identity_metadata(
    hostname,
    &container.id,
    &container.name,
    stream,
    container.image.as_deref(),
    &container.labels,
);
```

If the current syslog pipeline cannot carry metadata as a structured field yet, encode it as a compact JSON prefix with an unambiguous marker in `msg`:

```rust
let msg = format!("[cortex-agent-docker-meta:{}] {}", metadata, msg);
```

Then add parser support in the receiver/enrichment path that extracts this prefix into `metadata_json` and strips it from `message`. Keep the marker internal and documented. If the receiver already has a metadata injection hook, use that instead of the prefix.

- [ ] **Step 5: Add adapter test from structured metadata**

Append to `src/db/entity_resolution_tests.rs`:

```rust
#[test]
fn structured_agent_docker_metadata_resolves_without_central_docker_uri() {
    let identity = AgentDockerIdentity {
        agent_host: "nashost".to_string(),
        container_id: "abcdef1234567890".to_string(),
        container_name: "plex".to_string(),
        compose_project: Some("plex".to_string()),
        compose_service: Some("plex".to_string()),
        image: Some("lscr.io/linuxserver/plex:latest".to_string()),
        stream: "stdout".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let observations = observations_from_agent_docker_identity(&identity);
    let decisions = resolve_observations(&observations);
    assert!(decisions.iter().any(|d| {
        d.entity_type == ENTITY_TYPE_SERVICE_INSTANCE && d.canonical_key == "nashost/plex"
    }));
}
```

- [ ] **Step 6: Document the Docker identity contract**

Update `docs/contracts/log-row-shape.md`, `docs/contracts/metadata-json-shape.md`, and `docs/contracts/source-kinds.md` with:

```text
Agent Docker identity source: agent-docker.
Structured metadata path: metadata_json.agent_docker.
Required fields: host, container_id, container_name, stream.
Optional fields: compose_project, compose_service, image.
Canonical resolver proof must use agent-docker structured metadata. docker:// and docker-event:// rows are not proof for the resolver-backed graph contract.
```

- [ ] **Step 7: Run Task 4 tests**

Run:

```bash
cargo test agent::docker_tests:: -- --nocapture
cargo test db::entity_resolution::tests::structured_agent_docker_metadata_resolves_without_central_docker_uri -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit Task 4**

```bash
git add src/agent/docker.rs src/agent/docker_tests.rs src/db/entity_resolution/adapters.rs src/db/entity_resolution_tests.rs docs/contracts/log-row-shape.md docs/contracts/metadata-json-shape.md docs/contracts/source-kinds.md openwiki/log-intelligence.md
git commit -m "feat: add structured agent docker identity"
```

---

### Task 5: Resolver-Backed Graph Projection

**Files:**
- Modify: `src/db/graph.rs`
- Modify: `src/db/graph_tests.rs`
- Modify: `src/db/graph_inventory.rs`
- Modify: `src/db/graph_inventory/sql.rs`
- Modify: `src/db/graph_inventory_tests.rs`
- Modify: `src/app/services/map_answers.rs`
- Modify: `src/app/services/map_findings/risky_mounts.rs`
- Modify: `src/app/services/map_tests.rs`
- Modify: `src/app/services/map_findings/risky_mounts_tests.rs`
- Modify: `src/app/services/graph_safety.rs`
- Modify: `docs/contracts/investigation-graph.md`
- Modify: `openwiki/inventory-graph.md`

**Interfaces:**
- Consumes: resolver observations/decisions from Tasks 2-4.
- Produces: graph projection rows using `logical_service`, `service_instance`, and `instance_of`; no canonical `service:*` topology output after rebuild.

- [ ] **Step 1: Claim the Bead**

Run:

```bash
bd update syslog-mcp-vkln9.5 --claim
```

Expected: command exits `0`.

- [ ] **Step 2: Write failing runtime graph projection test**

Append to `src/db/graph_tests.rs`:

```rust
#[test]
fn graph_projection_emits_service_instance_not_nested_service_key() {
    let _guard = GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("resolver-graph-projection.db"),
    ))
    .unwrap();
    insert_logs_batch(
        &pool,
        &[LogBatchEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            hostname: "nashost".to_string(),
            facility: None,
            severity: "info".to_string(),
            app_name: Some("plex/plex/plex".to_string()),
            process_id: None,
            message: "Plex started".to_string(),
            raw: "Plex started".to_string(),
            source_ip: "10.0.0.1:514".to_string(),
            docker_checkpoint: None,
            ai_tool: None,
            ai_project: None,
            ai_session_id: None,
            ai_transcript_path: None,
            metadata_json: Some(r#"{"source_kind":"agent-docker","agent_docker":{"host":"nashost","container_id":"abcdef1234567890","container_name":"plex","compose_project":"plex","compose_service":"plex","stream":"stdout"}}"#.to_string()),
            http_status: None,
            auth_outcome: None,
            dns_blocked: None,
            event_action: None,
            parse_error: None,
        }],
    )
    .unwrap();
    refresh_graph_projection(&pool).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'logical_service' AND canonical_key = 'plex'"),
        1
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service_instance' AND canonical_key = 'nashost/plex'"),
        1
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service' AND canonical_key IN ('nashost:plex', 'nashost:plex:plex')"),
        0
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'app' AND canonical_key = 'plex/plex/plex'"),
        0
    );
}
```

- [ ] **Step 3: Run projection test and verify failure**

Run:

```bash
cargo test db::graph::tests::graph_projection_emits_service_instance_not_nested_service_key -- --nocapture
```

Expected: FAIL because current projection emits `service` and raw nested `app` topology.

- [ ] **Step 4: Update runtime graph extraction**

In `src/db/graph.rs`, replace the service-emitting part of `extract_docker_log_row` with:

```rust
let observations = agent_docker_observations_from_log_row(row);
let decisions = crate::db::entity_resolution::resolve_observations(&observations);
project_resolver_decisions(conn, row, &decisions)?;
```

Add helper:

```rust
fn project_resolver_decisions(
    conn: &rusqlite::Connection,
    row: &LogGraphRow,
    decisions: &[crate::db::entity_resolution::ResolvedEntityDecision],
) -> Result<()> {
    let source_id = row.id.to_string();
    let mut logical_ids = std::collections::BTreeMap::new();
    let mut instance_ids = std::collections::BTreeMap::new();
    for decision in decisions {
        let entity_id = ensure_entity(
            conn,
            decision.entity_type,
            &decision.canonical_key,
            &decision.display_label,
            SOURCE_KIND_LOG,
            &source_id,
            trust_to_graph(decision.trust),
            Some(&row.timestamp),
            Some(&row.timestamp),
        )?;
        if decision.entity_type == ENTITY_TYPE_LOGICAL_SERVICE {
            logical_ids.insert(decision.canonical_key.clone(), entity_id);
        } else if decision.entity_type == ENTITY_TYPE_SERVICE_INSTANCE {
            instance_ids.insert(decision.canonical_key.clone(), entity_id);
        }
    }
    for (instance_key, instance_id) in instance_ids {
        if let Some((_, service)) = crate::db::entity_resolution::split_service_instance_key(&instance_key) {
            if let Some(logical_id) = logical_ids.get(service) {
                ensure_relationship_with_evidence(
                    conn,
                    instance_id,
                    *logical_id,
                    REL_INSTANCE_OF,
                    REASON_RESOLVER_INSTANCE_OF,
                    TRUST_VERIFIED,
                    1.0,
                    EvidenceInput {
                        evidence_key: evidence_bucket_key("log", row.id, REASON_RESOLVER_INSTANCE_OF, &row.timestamp),
                        source_kind: SOURCE_KIND_LOG,
                        source_id: &source_id,
                        source_log_id: Some(row.id),
                        source_heartbeat_id: None,
                        source_signature_hash: None,
                        observed_at: &row.timestamp,
                        reason_text: Some("resolver linked service instance to logical service"),
                        confidence_delta: 1.0,
                        trust_level: TRUST_VERIFIED,
                        safe_excerpt: Some(&instance_key),
                        metadata_path: Some("metadata_json.agent_docker"),
                    },
                )?;
            }
        }
    }
    Ok(())
}
```

Implement `agent_docker_observations_from_log_row` so it reads `metadata_json.agent_docker` and ignores `docker://` / `docker-event://` as resolver proof.

- [ ] **Step 5: Write failing inventory projection test**

Append to `src/db/graph_inventory_tests.rs` a Plex inventory fixture:

```rust
#[test]
fn inventory_projection_links_service_instance_to_host_storage_compose_and_route() {
    let _guard = graph::GRAPH_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("inventory-service-instance.db"),
    ))
    .unwrap();
    let mut inventory = HomelabInventory::empty(
        "plex-proof".to_string(),
        "2026-01-01T00:00:00Z".to_string(),
    );
    inventory.nodes.push(InventoryNode {
        id: "node:nashost".to_string(),
        hostname: "nashost".to_string(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:nashost", "source_inventory"),
        roles: Vec::new(),
        ips: vec!["198.51.100.2".to_string()],
        os: Some("Unraid".to_string()),
        cpu: None,
        memory: None,
        listeners: Vec::new(),
        storage: Vec::new(),
        extras: Default::default(),
    });
    inventory.services.push(InventoryService {
        id: "service:nashost:plex".to_string(),
        name: "plex".to_string(),
        host: Some("nashost".to_string()),
        image: Some("lscr.io/linuxserver/plex:latest".to_string()),
        ports: vec![PortMapping { host: Some(32400), container: Some(32400), protocol: Some("tcp".to_string()) }],
        domains: vec!["plex.example.internal".to_string()],
        mounts: Vec::new(),
        trust_level: TrustLevel::Observed,
        provenance: provenance("ssh:nashost", "app_inventory"),
        extras: Default::default(),
    });
    project_inventory(&pool, &inventory).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service_instance' AND canonical_key = 'nashost/plex'"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service'"), 0);
}
```

Adjust field names to the actual `InventoryService` struct if the compile error points to different names; keep the assertions exactly about `service_instance` and absence of `service`.

- [ ] **Step 6: Convert inventory projection**

In `src/db/graph_inventory.rs`, replace `graph::ENTITY_TYPE_SERVICE` entities with resolver decisions from `observations_from_inventory_service`. Ensure these edges are emitted:

```text
service_instance:nashost/plex instance_of logical_service:plex
service_instance:nashost/plex runs_on host:nashost
compose_project:nashost/plex defines_service service_instance:nashost/plex
reverse_proxy:<route> routes_to service_instance:nashost/plex
service_instance:nashost/plex mounts storage:<safe-key>
```

Delete or stop using `service_key(service)` from `src/db/graph_inventory/sql.rs`.

- [ ] **Step 7: Route map/findings through service-instance keys**

In `src/app/services/map_answers.rs`, replace `service_dependency_key` with:

```rust
fn service_dependency_key(host: Option<&str>, service: Option<&str>) -> ServiceResult<String> {
    let service = required_map_target(service, "service", "service_dependencies")?;
    if crate::db::entity_resolution::classify_legacy_shape(&service).is_some() {
        return Err(ServiceError::InvalidInput(format!(
            "unsupported legacy graph service identity `{service}`: rejected_legacy_shape"
        )));
    }
    if service.contains('/') {
        return Ok(service.to_string());
    }
    let host = required_map_target(host, "host", "service_dependencies")?;
    crate::db::entity_resolution::service_instance_key(host, service.as_str()).ok_or_else(|| {
        ServiceError::InvalidInput("service_dependencies requires a non-empty host and service".into())
    })
}
```

Change the `service_dependencies` target entity type from `"service"` to `"service_instance"`.

In `src/app/services/map_findings/risky_mounts.rs`, replace `canonical_service_key(host, name)` with `service_instance_key(host, name).unwrap_or_else(|| format!("{host}/{name}"))` and keep it as a service-instance key.

- [ ] **Step 8: Strengthen graph identifier sanitization**

In `src/app/services/graph_safety.rs`, apply `redact_graph_text` to `GraphEntity.canonical_key`, `GraphEntity.display_label`, `GraphEntity.source_id`, `GraphRelationship.relationship_key`, and `GraphEntityCandidate.alias_key` when shaping public graph responses. Add tests in `src/app/services/graph.rs` or existing graph service tests that secret-like `source_id` and `/home/...` labels are redacted.

- [ ] **Step 9: Run Task 5 tests**

Run:

```bash
cargo test db::graph::tests::graph_projection_emits_service_instance_not_nested_service_key -- --nocapture
cargo test db::graph_inventory::tests::inventory_projection_links_service_instance_to_host_storage_compose_and_route -- --nocapture
cargo test app::services::map -- --nocapture
cargo test app::services::map_findings::risky_mounts -- --nocapture
```

Expected: all PASS.

- [ ] **Step 10: Commit Task 5**

```bash
git add src/db/graph.rs src/db/graph_tests.rs src/db/graph_inventory.rs src/db/graph_inventory/sql.rs src/db/graph_inventory_tests.rs src/app/services/map_answers.rs src/app/services/map_findings/risky_mounts.rs src/app/services/map_tests.rs src/app/services/map_findings/risky_mounts_tests.rs src/app/services/graph_safety.rs docs/contracts/investigation-graph.md openwiki/inventory-graph.md
git commit -m "feat: project graph through resolved service entities"
```

---

### Task 6: Resolved Graph and Topic Lookup Surfaces

**Files:**
- Modify: `src/db/queries.rs`
- Modify: `src/db/queries_graph_tests.rs`
- Modify: `src/app/models/ai_incidents.rs`
- Modify: `src/app/services/topic_correlate.rs`
- Modify: `src/app/services/topic_correlate_tests.rs`
- Modify: `src/app/services/graph.rs`
- Modify: `src/mcp/actions.rs`
- Modify: `src/mcp/tools.rs`
- Modify: `src/mcp/schemas.rs`
- Modify: `src/mcp/schemas_tests.rs`
- Modify: `src/cli/output/graph.rs`
- Modify: `src/cli/output/graph_tests.rs`
- Modify: `docs/mcp/TOOLS.md`
- Modify: `docs/mcp/SCHEMA.md`
- Modify: `docs/CLI.md`

**Interfaces:**
- Consumes: service-instance graph output from Task 5.
- Produces: resolver-backed topic lookup, service-specific log fan-out, inclusion metadata, and old-key rejection on public graph/topic surfaces.

- [ ] **Step 1: Claim the Bead**

Run:

```bash
bd update syslog-mcp-vkln9.6 --claim
```

Expected: command exits `0`.

- [ ] **Step 2: Write failing topic-correlate tests**

Append to `src/app/services/topic_correlate_tests.rs`:

```rust
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_plex_uses_service_instance_without_host_wide_fanout() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, pool, _dir) = test_service();
    insert_logs_batch(
        &pool,
        &[
            syslog("2026-01-01T00:00:00Z", "nashost", "kernel"),
            LogBatchEntry {
                timestamp: "2026-01-01T00:01:00Z".to_string(),
                hostname: "nashost".to_string(),
                facility: None,
                severity: "info".to_string(),
                app_name: Some("plex/plex/plex".to_string()),
                process_id: None,
                message: "Plex library scan".to_string(),
                raw: "Plex library scan".to_string(),
                source_ip: "10.0.0.1:514".to_string(),
                docker_checkpoint: None,
                ai_tool: None,
                ai_project: None,
                ai_session_id: None,
                ai_transcript_path: None,
                metadata_json: Some(r#"{"source_kind":"agent-docker","agent_docker":{"host":"nashost","container_id":"abcdef1234567890","container_name":"plex","compose_project":"plex","compose_service":"plex","stream":"stdout"}}"#.to_string()),
                http_status: None,
                auth_outcome: None,
                dns_blocked: None,
                event_action: None,
                parse_error: None,
            },
        ],
    )
    .unwrap();
    crate::db::graph::refresh_graph_projection(&pool).unwrap();
    let resp = svc.topic_correlate(TopicCorrelateRequest {
        topic: "plex".to_string(),
        limit: Some(10),
        ..Default::default()
    }).await.unwrap();
    assert!(resp.resolved_entities.iter().any(|e| {
        e.entity_type == "logical_service" && e.key == "plex"
    }));
    assert!(resp.timeline.iter().any(|row| row.message.contains("Plex library scan")));
    assert!(!resp.timeline.iter().any(|row| row.app_name.as_deref() == Some("kernel")));
    assert!(resp.timeline.iter().all(|row| {
        row.inclusion_reason.as_deref() == Some("service_instance")
            || row.fallback_kind.as_deref() == Some("explicit_degraded_host_context")
    }));
}

#[tokio::test]
async fn topic_rejects_legacy_service_shapes() {
    let (svc, _pool, _dir) = test_service();
    for topic in ["nashost:plex", "nashost:plex:plex", "plex/plex/plex"] {
        let err = svc.topic_correlate(TopicCorrelateRequest {
            topic: topic.to_string(),
            ..Default::default()
        }).await.unwrap_err();
        assert!(err.to_string().contains("rejected_legacy_shape"));
    }
}
```

- [ ] **Step 3: Add inclusion metadata fields**

Modify `TopicTimelineEntry` in `src/app/models/ai_incidents.rs`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub inclusion_reason: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub resolver_status: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub fallback_kind: Option<String>,
```

Add `resolver_status` to `ResolvedTopicEntity`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub resolver_status: Option<String>,
```

- [ ] **Step 4: Run topic tests and verify failure**

Run:

```bash
cargo test app::services::topic_correlate_tests:: -- --nocapture
```

Expected: FAIL because current topic resolution still uses prefix/label matching and host fan-out.

- [ ] **Step 5: Replace host-splitting fan-out**

In `src/db/queries.rs`, replace the mapping comment and implementation for service/container fan-out. Add a new result type:

```rust
#[derive(Debug, Clone)]
pub struct GraphRelatedLogEntry {
    pub entry: LogEntry,
    pub inclusion_reason: String,
    pub resolver_status: String,
    pub fallback_kind: Option<String>,
}
```

Add function:

```rust
pub fn search_logs_for_service_instances(
    pool: &DbPool,
    service_instance_keys: &[String],
    since: Option<&str>,
    until: Option<&str>,
    source_kinds: Option<&[SourceKind]>,
    limit: usize,
) -> Result<Vec<GraphRelatedLogEntry>> {
    if service_instance_keys.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 1000);
    let conn = pool.get()?;
    let mut predicates = Vec::new();
    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
    for key in service_instance_keys {
        let Some((host, service)) = crate::db::entity_resolution::split_service_instance_key(key) else {
            continue;
        };
        predicates.push("(l.hostname = ? AND (l.app_name = ? OR l.app_name LIKE ? OR json_extract(l.metadata_json, '$.agent_docker.compose_service') = ?))".to_string());
        bindings.push(host.to_string().into());
        bindings.push(service.to_string().into());
        bindings.push(format!("{service}/%").into());
        bindings.push(service.to_string().into());
    }
    if predicates.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = format!(
        "SELECT {FTS_SELECT_COLS}
           FROM logs l
          WHERE ({})
          ORDER BY l.timestamp DESC, l.id DESC
          LIMIT ?",
        predicates.join(" OR ")
    );
    add_time_and_source_kind_filters(&mut sql, &mut bindings, since, until, source_kinds);
    bindings.push((limit as i64).into());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), map_row)?;
    rows.map(|row| {
        row.map(|entry| GraphRelatedLogEntry {
            entry,
            inclusion_reason: "service_instance".to_string(),
            resolver_status: "resolved".to_string(),
            fallback_kind: None,
        })
    })
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(Into::into)
}
```

If `add_time_and_source_kind_filters` does not exist, factor the existing time/source-kind SQL from `search_logs_from_graph_related_entities` into that helper in this task.

- [ ] **Step 6: Use resolver-backed topic seeds**

In `src/app/services/topic_correlate.rs`, before splitting topic terms, reject legacy shapes:

```rust
let diagnostic = db::entity_resolution::diagnose_lookup_input(&req.topic);
if diagnostic.status == db::entity_resolution::ResolverStatus::RejectedLegacyShape {
    return Err(ServiceError::InvalidInput(format!(
        "unsupported legacy graph service identity `{}`: rejected_legacy_shape",
        req.topic
    )));
}
```

For bare topic terms, prefer exact resolver entity lookup:

```rust
let exact_service_key = db::entity_resolution::logical_service_key(&req.topic);
```

Resolve exact `logical_service` and its `service_instance` neighbors before label/prefix matching. Only use label/prefix matches as weak candidates with `resolver_status = "ambiguous"` and no automatic host fan-out.

When building timeline entries, populate:

```rust
inclusion_reason: Some(related.inclusion_reason),
resolver_status: Some(related.resolver_status),
fallback_kind: related.fallback_kind,
```

- [ ] **Step 7: Add graph walk caps**

Modify `graph_walk_n_hops` or add `graph_walk_service_topic` in `src/db/graph.rs` with:

```rust
pub const GRAPH_SERVICE_TOPIC_ENTITY_CAP: usize = 250;
pub const GRAPH_SERVICE_TOPIC_HOP_CAP: usize = 50;
```

For service-topic lookups, only traverse relationships needed for the proof:

```text
instance_of, runs_on, defines_service, routes_to, exposes_domain, mounts, has_artifact, matches_signature, worked_on
```

Do not traverse from service instance to all host logs by default.

- [ ] **Step 8: Update MCP/CLI schema docs**

In `src/mcp/schemas.rs`, change graph entity enum to include `logical_service` and `service_instance`. Remove wording that `service` is a supported service identity input. In `docs/mcp/SCHEMA.md`, `docs/mcp/TOOLS.md`, and `docs/CLI.md`, show:

```bash
cortex graph --mode around --entity-type logical_service --key plex
cortex graph --mode around --entity-type service_instance --key nashost/plex
```

Document that `nashost:plex` and `nashost:plex:plex` return `rejected_legacy_shape`.

- [ ] **Step 9: Run Task 6 tests**

Run:

```bash
cargo test db::queries_graph_tests:: -- --nocapture
cargo test app::services::topic_correlate_tests:: -- --nocapture
cargo test mcp::schemas_tests:: -- --nocapture
cargo test cli::output::graph_tests:: -- --nocapture
```

Expected: PASS.

- [ ] **Step 10: Commit Task 6**

```bash
git add src/db/queries.rs src/db/queries_graph_tests.rs src/app/models/ai_incidents.rs src/app/services/topic_correlate.rs src/app/services/topic_correlate_tests.rs src/app/services/graph.rs src/mcp/actions.rs src/mcp/tools.rs src/mcp/schemas.rs src/mcp/schemas_tests.rs src/cli/output/graph.rs src/cli/output/graph_tests.rs docs/mcp/TOOLS.md docs/mcp/SCHEMA.md docs/CLI.md
git commit -m "feat: resolve graph topics through service instances"
```

---

### Task 7: Plex Proof Workflow, Docs, and Final Validation

**Files:**
- Create: `scripts/validate-canonical-plex-graph.sh`
- Modify: `openwiki/inventory-graph.md`
- Modify: `openwiki/quickstart.md`
- Modify: `docs/contracts/investigation-graph.md`
- Modify: `docs/mcp/TOOLS.md`
- Modify: `docs/mcp/SCHEMA.md`
- Modify: `docs/CLI.md`
- Modify: `README.md`
- Modify: `src/db/graph_tests.rs`
- Modify: `src/app/services/topic_correlate_tests.rs`

**Interfaces:**
- Consumes: all previous task outputs.
- Produces: executable validation workflow and docs that prove the Plex scenario without relying on central Docker pull rows.

- [ ] **Step 1: Claim the Bead**

Run:

```bash
bd update syslog-mcp-vkln9.7 --claim
```

Expected: command exits `0`.

- [ ] **Step 2: Write the validation script**

Create `scripts/validate-canonical-plex-graph.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

db_path="${CORTEX_DB_PATH:-/home/jmagar/.cortex/data/cortex.db}"
mode="${1:-read-only}"

if [ "$mode" != "read-only" ]; then
  echo "Refusing live rebuild in validation script. Run read-only checks first, create a WAL-safe backup, and use documented operator commands for rebuild." >&2
  exit 2
fi

if [ ! -f "$db_path" ]; then
  echo "Cortex DB not found: $db_path" >&2
  exit 1
fi

old_count="$(sqlite3 "$db_path" "
SELECT COUNT(*)
  FROM graph_entities
 WHERE (entity_type = 'service' AND canonical_key IN ('nashost:plex', 'nashost:plex:plex'))
    OR (entity_type = 'app' AND canonical_key = 'plex/plex/plex');
")"
echo "old_key_count=$old_count"

new_count="$(sqlite3 "$db_path" "
SELECT COUNT(*)
  FROM graph_entities
 WHERE (entity_type = 'logical_service' AND canonical_key = 'plex')
    OR (entity_type = 'service_instance' AND canonical_key IN ('nashost/plex', 'backuphost/plex'));
")"
echo "new_key_count=$new_count"

sqlite3 "$db_path" "
EXPLAIN QUERY PLAN
SELECT id, entity_type, canonical_key
  FROM graph_entities
 WHERE entity_type IN ('logical_service', 'service_instance')
   AND canonical_key IN ('plex', 'nashost/plex');
"

echo "Read-only validation complete. old_key_count must be 0 after rebuild; new_key_count must be greater than 0 after resolver projection."
```

Run:

```bash
chmod +x scripts/validate-canonical-plex-graph.sh
```

- [ ] **Step 3: Add fixture proof tests**

In `src/db/graph_tests.rs`, add one combined fixture test that inserts:

```text
agent-docker Plex row for nashost
agent-docker Plex row for backuphost
raw syslog app row complex on nashost
raw syslog app row plex-backup on nashost
AI command row with project path mentioning plex
```

Assertions:

```rust
assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'logical_service' AND canonical_key = 'plex'"), 1);
assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service_instance' AND canonical_key = 'nashost/plex'"), 1);
assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service_instance' AND canonical_key = 'backuphost/plex'"), 1);
assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE canonical_key IN ('nashost:plex', 'nashost:plex:plex', 'plex/plex/plex')"), 0);
assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE canonical_key = 'complex' AND entity_type = 'logical_service'"), 0);
assert_eq!(count(&conn, "SELECT COUNT(*) FROM graph_entities WHERE canonical_key = 'plex-backup' AND entity_type = 'logical_service'"), 0);
```

- [ ] **Step 4: Add production-proof documentation**

Update `openwiki/inventory-graph.md` with a section named `Canonical Resolver Proof: Plex` containing:

```markdown
The canonical graph shape for Plex is:

- `logical_service:plex`
- `service_instance:nashost/plex`
- `service_instance:nashost/plex instance_of logical_service:plex`
- `service_instance:nashost/plex runs_on host:nashost`
- `compose_project:nashost/plex defines_service service_instance:nashost/plex`
- route/domain/storage/container/error/session evidence links to the service instance when deterministic evidence exists

`nashost:plex`, `nashost:plex:plex`, and `plex/plex/plex` are not supported service identity inputs. They are stale defect shapes.
```

Add read-only proof commands:

```bash
scripts/validate-canonical-plex-graph.sh
cortex graph --mode around --entity-type logical_service --key plex
cortex graph --mode around --entity-type service_instance --key nashost/plex
cortex topic-correlate plex --limit 20
```

State that central Docker pull rows are not proof for this milestone; the proof source is `metadata_json.agent_docker` from host-local agents.

- [ ] **Step 5: Update README and MCP/CLI docs**

In `README.md`, add a short operator-facing example:

```markdown
Searching `plex` resolves the logical service first, then concrete service instances such as `nashost/plex`. Cortex no longer treats `nashost:plex` or `nashost:plex:plex` as canonical service identities.
```

In `docs/mcp/TOOLS.md`, `docs/mcp/SCHEMA.md`, and `docs/CLI.md`, mirror the commands from Step 4 and mention `rejected_legacy_shape`.

- [ ] **Step 6: Run proof tests and docs checks**

Run:

```bash
cargo test db::graph::tests:: -- --nocapture
cargo test app::services::topic_correlate_tests:: -- --nocapture
bash scripts/check-public-identity.sh
bash scripts/validate-canonical-plex-graph.sh
bd swarm validate syslog-mcp-vkln9
```

Expected:
- Rust tests PASS.
- Public identity scan prints `OK`.
- Validation script prints counts and exits `0`.
- Beads swarm reports `Swarmable: YES`.

- [ ] **Step 7: Run final quality gates**

Run:

```bash
cargo fmt --check
cargo test
cargo clippy
```

Expected: all PASS. If full `cargo test` is too costly during an interactive run, run the focused test commands from Tasks 1-7 plus `cargo test --lib`, and record the omitted scope in the Beads close note.

- [ ] **Step 8: Close Beads and Commit**

Run:

```bash
bd close syslog-mcp-vkln9.1 --reason "Vocabulary/schema/cutover contract implemented and tested"
bd close syslog-mcp-vkln9.2 --reason "Resolver observations implemented and tested"
bd close syslog-mcp-vkln9.3 --reason "Resolver decisions and diagnostics implemented and tested"
bd close syslog-mcp-vkln9.4 --reason "Agent-first Docker identity implemented and tested"
bd close syslog-mcp-vkln9.5 --reason "Graph projection consumes resolver output"
bd close syslog-mcp-vkln9.6 --reason "Graph/topic lookup surfaces use resolved entities"
bd close syslog-mcp-vkln9.7 --reason "Plex proof workflow documented and validated"
bd close syslog-mcp-vkln9 --reason "Canonical entity-resolution milestone implemented and validated"
bd dolt commit -m "close canonical entity resolution swarm"
bd dolt push
git add scripts/validate-canonical-plex-graph.sh openwiki/inventory-graph.md openwiki/quickstart.md docs/contracts/investigation-graph.md docs/mcp/TOOLS.md docs/mcp/SCHEMA.md docs/CLI.md README.md src/db/graph_tests.rs src/app/services/topic_correlate_tests.rs
git commit -m "docs: add canonical plex graph proof workflow"
```

---

## Execution Notes

- The planned implementation intentionally keeps `ENTITY_TYPE_SERVICE` available during early schema migration work so old populated DBs can start, clean, and rebuild. Public service identity behavior is removed by projection/query tasks, not by a brittle first migration that fails on old rows.
- Central Docker code can remain in the repo if existing tests or operators still need it, but resolver/projection proof must not depend on `docker://` or `docker-event://`.
- If live validation discovers old rows after deployment, run a WAL-safe backup before any cleanup/rebuild:

```bash
cortex db backup
cortex graph rebuild
scripts/validate-canonical-plex-graph.sh
```

- Do not add a graph database, fuzzy matching, typo tolerance, browser workspace UI, broad historical backfill, or user/device service linkage without deterministic evidence as part of this milestone.

## Self-Review

Spec coverage:
- Hard-break vocabulary and schema contract: Task 1.
- Bounded observation extraction: Task 2.
- Deterministic resolver decisions and diagnostics: Task 3.
- Agent-first Docker parity/proof source: Task 4.
- Resolver-backed projection and stale-row cleanup: Task 5.
- Resolved graph/topic lookup and no host-wide service fan-out: Task 6.
- Plex proof workflow and docs: Task 7.
- Privacy/redaction: Tasks 2, 5, 6, and 7.
- Production read-only proof and rebuild constraints: Task 7.

Red-flag scan:
- The plan avoids deferred implementation markers inside task steps.
- Every code-changing task starts with a concrete failing test and includes focused commands.
- Old key shapes are always rejection/removal assertions, never compatibility assertions.

Type consistency:
- `logical_service_key`, `service_instance_key`, `ResolverObservation`, `AgentDockerIdentity`, `ResolverStatus`, `ResolvedEntityDecision`, and `ResolverDiagnostic` are defined before later tasks consume them.
- Topic response fields `inclusion_reason`, `resolver_status`, and `fallback_kind` are added before topic service tests require them.
