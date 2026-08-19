//! Pure adapters that convert source rows (agent Docker identity, raw log
//! app labels, verified inventory services) into bounded resolver
//! observations. Adapters never touch the database.

use crate::agent::docker::AGENT_DOCKER_SOURCE_KIND;
use crate::inventory::schema::{InventoryService, TrustLevel};

use super::observation::*;
use super::vocab::{logical_service_key, service_instance_key};

/// Convert structured agent-attested Docker identity into observations.
/// The compose service label (falling back to the container name) is the
/// logical service identity; the agent host scopes the service instance.
pub fn observations_from_agent_docker_identity(
    identity: &AgentDockerIdentity,
) -> Vec<ResolverObservation> {
    let Some(host_key) = logical_service_key(&identity.agent_host) else {
        return Vec::new();
    };
    // Evidence-path attribution follows the actual source of the service
    // name: the compose service label when present, else the container name.
    let (service_name, service_evidence_path) = match identity.compose_service.as_deref() {
        Some(compose_service) => (compose_service, "agent_docker.compose_service"),
        None => (
            identity.container_name.as_str(),
            "agent_docker.container_name",
        ),
    };
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
            source_kind: AGENT_DOCKER_SOURCE_KIND.to_string(),
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
            source_kind: AGENT_DOCKER_SOURCE_KIND.to_string(),
            source_id: identity.container_id.clone(),
            evidence_path: service_evidence_path.to_string(),
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
            source_kind: AGENT_DOCKER_SOURCE_KIND.to_string(),
            source_id: identity.container_id.clone(),
            // The instance key is host + service name (compose service label
            // or container-name fallback); `compose_project` is never read.
            evidence_path: "agent_docker.host_service".to_string(),
            observed_at: identity.observed_at.clone(),
            trust: ResolverTrust::Verified,
            structured: true,
        },
    ]
}

/// Convert a raw observed log app label into a single weak observation.
/// Raw labels never produce `LogicalService` / `ServiceInstance`
/// observations on their own — they must be matched to structured evidence
/// by the resolver, or they stay raw.
// Test-only contract coverage: pins the plan-locked "raw labels never
// self-upgrade" resolver rule (entity_resolution_tests.rs).
#[cfg(test)]
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

/// Convert a verified/observed inventory service into observations: the
/// logical service always (when the name canonicalizes), plus the host,
/// service instance, and domain/mount (storage) context when the inventory
/// row carries a host. A hostless service still asserts logical identity —
/// deployment topology is simply absent, never guessed into an `unknown/`
/// instance.
pub fn observations_from_inventory_service(service: &InventoryService) -> Vec<ResolverObservation> {
    let Some(logical_key) = logical_service_key(&service.name) else {
        return Vec::new();
    };
    let trust = inventory_trust(&service.trust_level);
    let source_kind = "app_inventory".to_string();
    let source_id = service.id.clone();
    let observed_at = service.provenance.collected_at.clone();
    let mut observations = vec![ResolverObservation {
        kind: ObservationKind::LogicalService,
        observed_key: logical_key.clone(),
        display_label: safe_display_value(&service.name),
        host_key: None,
        logical_service_key: Some(logical_key.clone()),
        service_instance_key: None,
        source_kind: source_kind.clone(),
        source_id: source_id.clone(),
        evidence_path: "inventory.services.name".to_string(),
        observed_at: observed_at.clone(),
        trust,
        structured: true,
    }];

    let host = service.host.as_deref();
    let host_key = host.and_then(logical_service_key);
    let instance_key = host_key
        .as_deref()
        .and_then(|host_key| service_instance_key(host_key, &logical_key));
    let (Some(host), Some(host_key), Some(instance_key)) = (host, host_key, instance_key) else {
        return observations;
    };

    observations.push(ResolverObservation {
        kind: ObservationKind::Host,
        observed_key: host_key.clone(),
        display_label: safe_display_value(host),
        host_key: Some(host_key.clone()),
        logical_service_key: None,
        service_instance_key: None,
        source_kind: source_kind.clone(),
        source_id: source_id.clone(),
        evidence_path: "inventory.services.host".to_string(),
        observed_at: observed_at.clone(),
        trust,
        structured: true,
    });
    observations.push(ResolverObservation {
        kind: ObservationKind::ServiceInstance,
        observed_key: instance_key.clone(),
        display_label: instance_key.clone(),
        host_key: Some(host_key.clone()),
        logical_service_key: Some(logical_key.clone()),
        service_instance_key: Some(instance_key.clone()),
        source_kind: source_kind.clone(),
        source_id: source_id.clone(),
        evidence_path: "inventory.services".to_string(),
        observed_at: observed_at.clone(),
        trust,
        structured: true,
    });
    for domain in &service.domains {
        let domain_key = domain.trim().to_ascii_lowercase();
        if domain_key.is_empty() {
            continue;
        }
        observations.push(ResolverObservation {
            kind: ObservationKind::Domain,
            observed_key: domain_key,
            display_label: safe_display_value(domain),
            host_key: Some(host_key.clone()),
            logical_service_key: Some(logical_key.clone()),
            service_instance_key: Some(instance_key.clone()),
            source_kind: source_kind.clone(),
            source_id: source_id.clone(),
            evidence_path: "inventory.services.domains".to_string(),
            observed_at: observed_at.clone(),
            trust,
            structured: true,
        });
    }
    for mount in &service.mounts {
        let target = mount.target.trim();
        if target.is_empty() {
            continue;
        }
        observations.push(ResolverObservation {
            kind: ObservationKind::Storage,
            observed_key: format!("{host_key}:{target}"),
            display_label: safe_display_value(target),
            host_key: Some(host_key.clone()),
            logical_service_key: Some(logical_key.clone()),
            service_instance_key: Some(instance_key.clone()),
            source_kind: source_kind.clone(),
            source_id: source_id.clone(),
            evidence_path: "inventory.services.mounts".to_string(),
            observed_at: observed_at.clone(),
            trust,
            structured: true,
        });
    }
    observations
}

fn inventory_trust(trust_level: &TrustLevel) -> ResolverTrust {
    match trust_level {
        TrustLevel::Verified | TrustLevel::Observed => ResolverTrust::Verified,
        TrustLevel::Claimed => ResolverTrust::Claimed,
        TrustLevel::Inferred => ResolverTrust::Inferred,
    }
}
