mod sql;
#[cfg(test)]
#[path = "graph_inventory_tests.rs"]
mod tests;

use std::collections::{BTreeMap, btree_map::Entry};

use anyhow::{Context, Result};

use crate::db::graph;
use crate::db::graph_resolver_projection::trust_to_graph;
use crate::db::{DbPool, entity_resolution, write_lock};
use crate::inventory::schema::HomelabInventory;

use self::sql::{
    add_alias, add_relationship, canonical, canonical_or_raw, graph_counts,
    prune_previous_inventory_projection, safe_inventory_source_id, scoped_inventory_key, trust,
    update_projection_meta, upsert_entity,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryGraphStats {
    pub source_row_count: i64,
    pub entity_count: i64,
    pub relationship_count: i64,
    pub evidence_count: i64,
}

pub fn project_inventory(
    pool: &DbPool,
    inventory: &HomelabInventory,
) -> Result<InventoryGraphStats> {
    let plan = build_projection_plan(inventory);
    warn_skipped_services(&plan);
    apply_projection_plan(pool, &plan, || {})
}

/// One warning per inventory projection listing the services whose identity
/// failed canonicalization and were therefore left out of the graph.
fn warn_skipped_services(plan: &InventoryProjectionPlan) {
    if !plan.skipped_service_ids.is_empty() {
        tracing::warn!(
            skipped = plan.skipped_service_ids.len(),
            service_ids = ?plan.skipped_service_ids,
            "inventory services skipped from graph projection: service name failed canonicalization"
        );
    }
}

#[cfg(test)]
fn project_inventory_with_apply_hook(
    pool: &DbPool,
    inventory: &HomelabInventory,
    before_apply: impl FnOnce(),
) -> Result<InventoryGraphStats> {
    let plan = build_projection_plan(inventory);
    warn_skipped_services(&plan);
    apply_projection_plan(pool, &plan, before_apply)
}

fn apply_projection_plan(
    pool: &DbPool,
    plan: &InventoryProjectionPlan,
    before_apply: impl FnOnce(),
) -> Result<InventoryGraphStats> {
    before_apply();
    let mut conn = pool.get().context("borrow sqlite connection")?;
    let _guard = write_lock();
    let tx = conn
        .transaction()
        .context("start inventory graph transaction")?;

    prune_previous_inventory_projection(&tx)?;

    // Heartbeats are the authoritative proof that a fleet host exists. Keep
    // them resolvable even when an SSH/Docker inventory collector fails and
    // therefore contributes no InventoryNode for that refresh.
    project_heartbeat_hosts(&tx)?;

    let mut entities = BTreeMap::new();
    for entity in &plan.entities {
        let entity_ref = upsert_entity(
            &tx,
            entity.key.kind,
            &entity.key.key,
            &entity.display_label,
            entity.source_kind,
            &entity.source_id,
            entity.trust_level,
            &entity.observed_at,
        )?;
        entities.insert(entity.key.clone(), entity_ref);
    }

    for alias in &plan.aliases {
        let entity = entities
            .get(&alias.entity)
            .with_context(|| format!("missing planned entity {}", alias.entity.key))?;
        add_alias(
            &tx,
            entity.id,
            alias.alias_type,
            &alias.alias_key,
            &alias.alias_value,
            alias.source_kind,
            alias.trust_level,
            &alias.observed_at,
        )?;
    }

    for relationship in &plan.relationships {
        let src = entities
            .get(&relationship.src)
            .with_context(|| format!("missing planned source entity {}", relationship.src.key))?;
        let dst = entities.get(&relationship.dst).with_context(|| {
            format!(
                "missing planned destination entity {}",
                relationship.dst.key
            )
        })?;
        add_relationship(
            &tx,
            src,
            dst,
            relationship.relationship_type,
            relationship.reason_code,
            relationship.source_kind,
            &relationship.source_id,
            &relationship.observed_at,
            relationship.trust_level,
            relationship.confidence,
            &relationship.safe_excerpt,
        )?;
    }

    let stats = graph_counts(&tx)?;
    update_projection_meta(&tx, &stats)?;
    tx.commit().context("commit inventory graph projection")?;
    Ok(stats)
}

fn project_heartbeat_hosts(conn: &rusqlite::Connection) -> Result<()> {
    let rows = {
        let mut stmt = conn.prepare(
            "SELECT heartbeat_id, host_id, hostname, sampled_at
               FROM host_heartbeats_latest
              ORDER BY hostname ASC",
        )?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (heartbeat_id, host_id, hostname, sampled_at) in rows {
        let Some(key) = canonical(&hostname) else {
            continue;
        };
        let entity = upsert_entity(
            conn,
            graph::ENTITY_TYPE_HOST,
            &key,
            &hostname,
            graph::SOURCE_KIND_HEARTBEAT,
            &heartbeat_id.to_string(),
            graph::TRUST_VERIFIED,
            &sampled_at,
        )?;
        add_alias(
            conn,
            entity.id,
            "hostname",
            &key,
            &hostname,
            graph::SOURCE_KIND_HEARTBEAT,
            graph::TRUST_VERIFIED,
            &sampled_at,
        )?;
        add_alias(
            conn,
            entity.id,
            "heartbeat_host_id",
            &canonical_or_raw(&host_id),
            &host_id,
            graph::SOURCE_KIND_HEARTBEAT,
            graph::TRUST_VERIFIED,
            &sampled_at,
        )?;
    }
    Ok(())
}

fn build_projection_plan(inventory: &HomelabInventory) -> InventoryProjectionPlan {
    let mut plan = InventoryProjectionPlan::default();
    let mut hosts = BTreeMap::new();
    for node in &inventory.nodes {
        let Some(key) = canonical(&node.hostname) else {
            continue;
        };
        let entity = plan.entity(
            graph::ENTITY_TYPE_HOST,
            &key,
            &node.hostname,
            graph::SOURCE_KIND_SOURCE_INVENTORY,
            &node.id,
            trust(&node.trust_level),
            &node.provenance.collected_at,
        );
        plan.alias(
            &entity,
            "hostname",
            &key,
            &node.hostname,
            graph::SOURCE_KIND_SOURCE_INVENTORY,
            trust(&node.trust_level),
            &node.provenance.collected_at,
        );
        for ip in &node.ips {
            if let Some(alias_key) = canonical(ip) {
                plan.alias(
                    &entity,
                    "ip",
                    &alias_key,
                    ip,
                    graph::SOURCE_KIND_SOURCE_INVENTORY,
                    trust(&node.trust_level),
                    &node.provenance.collected_at,
                );
            }
        }
        hosts.insert(key, entity);
    }

    let mut services = BTreeMap::new();
    let mut unique_service_aliases = BTreeMap::new();
    let mut logical_services: BTreeMap<String, PlannedEntityKey> = BTreeMap::new();
    for service in &inventory.services {
        // Canonical service identity (entity_resolution_v2): inventory
        // services flow through the shared resolver adapter, projecting as
        // `logical_service` (`plex`) plus a host-scoped `service_instance`
        // (`nashost/plex`). Legacy `service` entities (`host:name`) are never
        // emitted.
        let observations = entity_resolution::observations_from_inventory_service(service);
        let decisions = entity_resolution::resolve_observations(&observations);
        let logical_decision = decisions
            .iter()
            .find(|d| d.entity_type == graph::ENTITY_TYPE_LOGICAL_SERVICE);
        let instance_decision = decisions
            .iter()
            .find(|d| d.entity_type == graph::ENTITY_TYPE_SERVICE_INSTANCE);
        let Some(logical_decision) = logical_decision else {
            plan.skipped_service_ids.push(service.id.clone());
            continue;
        };
        let logical_key = logical_decision.canonical_key.clone();
        let logical_entity = match logical_services.entry(logical_key.clone()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let entity = plan.entity(
                    graph::ENTITY_TYPE_LOGICAL_SERVICE,
                    &logical_key,
                    &service.name,
                    graph::SOURCE_KIND_APP_INVENTORY,
                    &service.id,
                    trust_to_graph(logical_decision.trust),
                    &service.provenance.collected_at,
                );
                entry.insert(entity.clone());
                entity
            }
        };

        let Some(instance_decision) = instance_decision else {
            // No host context: the logical service exists, but there is no
            // deployment topology to assert. Ambiguity stays visible instead
            // of being guessed into an `unknown/` instance.
            insert_unique_alias(
                &mut unique_service_aliases,
                canonical_or_raw(&service.name),
                logical_entity.clone(),
            );
            continue;
        };
        let instance_key = instance_decision.canonical_key.clone();
        let service_entity = plan.entity(
            graph::ENTITY_TYPE_SERVICE_INSTANCE,
            &instance_key,
            &instance_key,
            graph::SOURCE_KIND_APP_INVENTORY,
            &service.id,
            trust_to_graph(instance_decision.trust),
            &service.provenance.collected_at,
        );
        plan.relationship(
            &service_entity,
            &logical_entity,
            graph::REL_INSTANCE_OF,
            graph::REASON_RESOLVER_INSTANCE_OF,
            graph::SOURCE_KIND_APP_INVENTORY,
            &service.id,
            &service.provenance.collected_at,
            trust_to_graph(instance_decision.trust),
            0.95,
            &format!("{} is an instance of {}", instance_key, logical_key),
        );
        services.insert(instance_key.clone(), service_entity.clone());
        insert_unique_alias(
            &mut unique_service_aliases,
            canonical_or_raw(&service.name),
            service_entity.clone(),
        );

        for domain in &service.domains {
            if let Some(alias_key) = canonical(domain) {
                plan.alias(
                    &service_entity,
                    "domain",
                    &alias_key,
                    domain,
                    graph::SOURCE_KIND_APP_INVENTORY,
                    trust(&service.trust_level),
                    &service.provenance.collected_at,
                );
            }
        }

        if let Some(host) = service
            .host
            .as_ref()
            .and_then(|h| hosts.get(&canonical_or_raw(h)))
        {
            plan.relationship(
                &service_entity,
                host,
                graph::REL_RUNS_ON,
                graph::REASON_INVENTORY_SERVICE,
                graph::SOURCE_KIND_APP_INVENTORY,
                &service.id,
                &service.provenance.collected_at,
                graph::TRUST_INFERRED,
                0.85,
                &format!("{} observed on {}", service.name, host.key),
            );
        }

        for mount in &service.mounts {
            let storage_key = canonical_or_raw(&format!(
                "{}:{}",
                service.host.as_deref().unwrap_or("unknown"),
                mount.target
            ));
            let storage = plan.entity(
                graph::ENTITY_TYPE_STORAGE,
                &storage_key,
                &mount.target,
                graph::SOURCE_KIND_APP_INVENTORY,
                &storage_key,
                graph::TRUST_INFERRED,
                &service.provenance.collected_at,
            );
            plan.relationship(
                &service_entity,
                &storage,
                graph::REL_MOUNTS,
                graph::REASON_STORAGE_PROBE,
                graph::SOURCE_KIND_APP_INVENTORY,
                &service.id,
                &service.provenance.collected_at,
                graph::TRUST_INFERRED,
                0.65,
                &format!("{} mounts {}", service.name, mount.target),
            );
        }
    }
    services.extend(
        unique_service_aliases
            .into_iter()
            .filter_map(|(key, service)| service.map(|service| (key, service))),
    );

    let mut artifacts = BTreeMap::new();
    for artifact in &inventory.artifact_refs {
        let display = format!("{} artifact {}", artifact.kind, artifact.id);
        let entity = plan.entity(
            graph::ENTITY_TYPE_CONFIG_ARTIFACT,
            &canonical_or_raw(&artifact.id),
            &display,
            graph::SOURCE_KIND_APP_INVENTORY,
            &artifact.id,
            graph::TRUST_VERIFIED,
            &inventory.generated_at,
        );
        artifacts.insert(artifact.id.clone(), entity.clone());
        if let Some(path) = &artifact.source_path {
            artifacts.insert(path.clone(), entity);
        }
    }

    for project in &inventory.compose_projects {
        let project_key = scoped_inventory_key(&project.provenance.source, &project.name);
        let project_source = safe_inventory_source_id(&project.provenance.source);
        let project_entity = plan.entity(
            graph::ENTITY_TYPE_COMPOSE_PROJECT,
            &project_key,
            &project.name,
            graph::SOURCE_KIND_APP_INVENTORY,
            &project_source,
            graph::TRUST_VERIFIED,
            &project.provenance.collected_at,
        );
        for service_name in &project.services {
            if let Some(service_entity) =
                match_service_name_key(service_name, &project.provenance.source, &services)
            {
                plan.relationship(
                    &project_entity,
                    service_entity,
                    graph::REL_DEFINES_SERVICE,
                    graph::REASON_COMPOSE_CONFIG,
                    graph::SOURCE_KIND_APP_INVENTORY,
                    &project_source,
                    &project.provenance.collected_at,
                    graph::TRUST_VERIFIED,
                    0.90,
                    &format!("compose project {} defines {}", project.name, service_name),
                );
            }
        }
        for compose_file in &project.compose_files {
            if let Some(artifact) = artifacts.get(compose_file) {
                let artifact_id = artifact.key.clone();
                plan.relationship(
                    &project_entity,
                    artifact,
                    graph::REL_HAS_ARTIFACT,
                    graph::REASON_CONFIG_ARTIFACT,
                    graph::SOURCE_KIND_APP_INVENTORY,
                    &project_source,
                    &project.provenance.collected_at,
                    graph::TRUST_VERIFIED,
                    0.95,
                    &format!("compose artifact {}", artifact_id),
                );
            }
        }
    }

    for route in &inventory.reverse_proxies {
        let proxy_key = canonical_or_raw(&route.id);
        let proxy = plan.entity(
            graph::ENTITY_TYPE_REVERSE_PROXY,
            &proxy_key,
            route.server_names.first().unwrap_or(&route.id),
            graph::SOURCE_KIND_APP_INVENTORY,
            &route.id,
            graph::TRUST_VERIFIED,
            &route.provenance.collected_at,
        );
        for domain in &route.server_names {
            let domain_key = canonical_or_raw(domain);
            let domain_entity = plan.entity(
                graph::ENTITY_TYPE_DOMAIN,
                &domain_key,
                domain,
                graph::SOURCE_KIND_APP_INVENTORY,
                &route.id,
                graph::TRUST_VERIFIED,
                &route.provenance.collected_at,
            );
            plan.relationship(
                &proxy,
                &domain_entity,
                graph::REL_EXPOSES_DOMAIN,
                graph::REASON_REVERSE_PROXY_CONFIG,
                graph::SOURCE_KIND_APP_INVENTORY,
                &route.id,
                &route.provenance.collected_at,
                graph::TRUST_VERIFIED,
                0.95,
                &format!("proxy exposes {}", domain),
            );
        }
        for upstream in &route.upstreams {
            if let Some(service) = match_upstream_key(upstream, &route.provenance.source, &services)
            {
                plan.relationship(
                    &proxy,
                    service,
                    graph::REL_ROUTES_TO,
                    graph::REASON_REVERSE_PROXY_CONFIG,
                    graph::SOURCE_KIND_APP_INVENTORY,
                    &route.id,
                    &route.provenance.collected_at,
                    graph::TRUST_VERIFIED,
                    0.85,
                    &format!("proxy routes to {}", upstream),
                );
            }
        }
    }

    for network in &inventory.networks {
        let network_source = safe_inventory_source_id(&network.provenance.source);
        let network_entity = plan.entity(
            graph::ENTITY_TYPE_NETWORK,
            &scoped_inventory_key(&network.provenance.source, &network.name),
            &network.name,
            graph::SOURCE_KIND_APP_INVENTORY,
            &network_source,
            graph::TRUST_VERIFIED,
            &network.provenance.collected_at,
        );
        for member in &network.members {
            if let Some(service) =
                match_service_name_key(member, &network.provenance.source, &services)
            {
                plan.relationship(
                    service,
                    &network_entity,
                    graph::REL_ATTACHED_TO,
                    graph::REASON_DOCKER_NETWORK,
                    graph::SOURCE_KIND_APP_INVENTORY,
                    &network_source,
                    &network.provenance.collected_at,
                    graph::TRUST_VERIFIED,
                    0.80,
                    &format!("{} attached to network {}", member, network.name),
                );
            }
        }
    }

    for storage in &inventory.storage {
        let entity = plan.entity(
            graph::ENTITY_TYPE_STORAGE,
            &canonical_or_raw(&storage.id),
            &storage.mount,
            graph::SOURCE_KIND_SOURCE_INVENTORY,
            &storage.id,
            graph::TRUST_VERIFIED,
            &storage.provenance.collected_at,
        );
        if let Some(host) = storage
            .id
            .split(':')
            .nth(1)
            .and_then(|host| hosts.get(&canonical_or_raw(host)))
        {
            plan.relationship(
                host,
                &entity,
                graph::REL_BACKED_BY,
                graph::REASON_STORAGE_PROBE,
                graph::SOURCE_KIND_SOURCE_INVENTORY,
                &storage.id,
                &storage.provenance.collected_at,
                graph::TRUST_VERIFIED,
                0.75,
                &format!("{} storage mounted at {}", host.key, storage.mount),
            );
        }
    }

    plan
}

pub fn mark_inventory_projection_failed(pool: &DbPool, error: &str) -> Result<()> {
    let conn = pool.get().context("borrow sqlite connection")?;
    let _guard = write_lock();
    sql::mark_projection_degraded(&conn, error)
}

fn insert_unique_alias(
    aliases: &mut BTreeMap<String, Option<PlannedEntityKey>>,
    key: String,
    service: PlannedEntityKey,
) {
    match aliases.entry(key) {
        Entry::Vacant(entry) => {
            entry.insert(Some(service));
        }
        Entry::Occupied(mut entry) => {
            if entry
                .get()
                .as_ref()
                .is_some_and(|existing| existing != &service)
            {
                entry.insert(None);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlannedEntityKey {
    kind: &'static str,
    key: String,
}

#[derive(Debug, Clone)]
struct EntityPlan {
    key: PlannedEntityKey,
    display_label: String,
    source_kind: &'static str,
    source_id: String,
    trust_level: &'static str,
    observed_at: String,
}

#[derive(Debug, Clone)]
struct AliasPlan {
    entity: PlannedEntityKey,
    alias_type: &'static str,
    alias_key: String,
    alias_value: String,
    source_kind: &'static str,
    trust_level: &'static str,
    observed_at: String,
}

#[derive(Debug, Clone)]
struct RelationshipPlan {
    src: PlannedEntityKey,
    dst: PlannedEntityKey,
    relationship_type: &'static str,
    reason_code: &'static str,
    source_kind: &'static str,
    source_id: String,
    observed_at: String,
    trust_level: &'static str,
    confidence: f64,
    safe_excerpt: String,
}

#[derive(Debug, Default)]
struct InventoryProjectionPlan {
    entities: Vec<EntityPlan>,
    aliases: Vec<AliasPlan>,
    relationships: Vec<RelationshipPlan>,
    /// Inventory service ids skipped because their names failed
    /// canonicalization (no logical-service decision). Surfaced by the
    /// caller so silently absent services are diagnosable.
    skipped_service_ids: Vec<String>,
}

impl InventoryProjectionPlan {
    #[allow(clippy::too_many_arguments)]
    fn entity(
        &mut self,
        kind: &'static str,
        key: &str,
        display_label: &str,
        source_kind: &'static str,
        source_id: &str,
        trust_level: &'static str,
        observed_at: &str,
    ) -> PlannedEntityKey {
        let key = PlannedEntityKey {
            kind,
            key: key.to_string(),
        };
        self.entities.push(EntityPlan {
            key: key.clone(),
            display_label: display_label.to_string(),
            source_kind,
            source_id: source_id.to_string(),
            trust_level,
            observed_at: observed_at.to_string(),
        });
        key
    }

    #[allow(clippy::too_many_arguments)]
    fn alias(
        &mut self,
        entity: &PlannedEntityKey,
        alias_type: &'static str,
        alias_key: &str,
        alias_value: &str,
        source_kind: &'static str,
        trust_level: &'static str,
        observed_at: &str,
    ) {
        self.aliases.push(AliasPlan {
            entity: entity.clone(),
            alias_type,
            alias_key: alias_key.to_string(),
            alias_value: alias_value.to_string(),
            source_kind,
            trust_level,
            observed_at: observed_at.to_string(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn relationship(
        &mut self,
        src: &PlannedEntityKey,
        dst: &PlannedEntityKey,
        relationship_type: &'static str,
        reason_code: &'static str,
        source_kind: &'static str,
        source_id: &str,
        observed_at: &str,
        trust_level: &'static str,
        confidence: f64,
        safe_excerpt: &str,
    ) {
        self.relationships.push(RelationshipPlan {
            src: src.clone(),
            dst: dst.clone(),
            relationship_type,
            reason_code,
            source_kind,
            source_id: source_id.to_string(),
            observed_at: observed_at.to_string(),
            trust_level,
            confidence,
            safe_excerpt: safe_excerpt.to_string(),
        });
    }
}

fn match_upstream_key<'a>(
    upstream: &str,
    source: &str,
    services: &'a BTreeMap<String, PlannedEntityKey>,
) -> Option<&'a PlannedEntityKey> {
    let normalized = canonical_or_raw(upstream);
    let prefix = upstream
        .split([':', '/', '@'])
        .find(|part| !part.is_empty() && !is_url_scheme_token(part))
        .map(canonical_or_raw);
    prefix
        .and_then(|key| match_service_name_key(&key, source, services))
        .or_else(|| match_service_name_key(&normalized, source, services))
}

fn is_url_scheme_token(part: &str) -> bool {
    part.eq_ignore_ascii_case("http") || part.eq_ignore_ascii_case("https")
}

fn match_service_name_key<'a>(
    name: &str,
    source: &str,
    services: &'a BTreeMap<String, PlannedEntityKey>,
) -> Option<&'a PlannedEntityKey> {
    source_host(source)
        .and_then(|host| entity_resolution::service_instance_key(host, name))
        .and_then(|key| services.get(&key))
        .or_else(|| services.get(&canonical_or_raw(name)))
}

fn source_host(source: &str) -> Option<&str> {
    let mut parts = source.split(':');
    let _collector = parts.next()?;
    let host = parts.next()?.trim();
    if host.is_empty() || host.starts_with('/') {
        None
    } else {
        Some(host)
    }
}
