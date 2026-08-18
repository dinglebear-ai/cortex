//! Canonical entity/relationship/reason vocabulary and key grammar for the
//! resolver-backed graph projection contract.

pub const ENTITY_TYPE_LOGICAL_SERVICE: &str = "logical_service";
pub const ENTITY_TYPE_SERVICE_INSTANCE: &str = "service_instance";
pub const REL_INSTANCE_OF: &str = "instance_of";
pub const REASON_RESOLVER_INSTANCE_OF: &str = "resolver_instance_of";
#[cfg(test)]
pub const REASON_RESOLVER_SERVICE_INSTANCE: &str = "resolver_service_instance";
#[cfg(test)]
pub const REASON_RESOLVER_RAW_APP_LABEL: &str = "resolver_raw_app_label";
pub const GRAPH_PROJECTION_CONTRACT_KEY: &str = "graph_projection_contract";
pub const GRAPH_PROJECTION_CONTRACT_V2: &str = "entity_resolution_v2";

/// Inclusion reasons annotating why a correlated log row was pulled in, and
/// the fallback kind marking the explicit degraded host-context path.
pub const INCLUSION_SERVICE_INSTANCE: &str = "service_instance";
pub const INCLUSION_GRAPH_RELATED: &str = "graph_related";
pub const INCLUSION_HOST_CONTEXT: &str = "host_context";
pub const FALLBACK_EXPLICIT_DEGRADED_HOST_CONTEXT: &str = "explicit_degraded_host_context";

/// Legacy (pre entity-resolution) service identity shapes. These are
/// classified so callers can reject them; they are never normalized into
/// canonical keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyShape {
    HostService,
    HostProjectService,
    SlashTriplet,
}

/// Canonical logical-service key: lowercased, trimmed, non-key characters
/// mapped to `-`. Kept characters are ASCII alphanumerics plus `-`, `_`,
/// and `.` (dots preserve raw hostnames like `nashost.lan`). Returns `None`
/// when nothing canonical remains.
pub fn logical_service_key(name: &str) -> Option<String> {
    canonical_component(name)
}

/// Canonical service-instance key `host/service`. Host and service are each
/// canonicalized independently; `None` when either side is empty.
pub fn service_instance_key(host: &str, service: &str) -> Option<String> {
    Some(format!(
        "{}/{}",
        canonical_component(host)?,
        canonical_component(service)?
    ))
}

/// Split a canonical `host/service` key. Rejects empty components and any
/// extra `/` segments (which would be a legacy slash-triplet shape).
///
/// This validates *shape*, not canonicality: components are not checked
/// against the canonical character set, so do not use this as an input
/// validator for untrusted keys.
pub fn split_service_instance_key(key: &str) -> Option<(&str, &str)> {
    let (host, service) = key.split_once('/')?;
    if host.is_empty() || service.is_empty() || service.contains('/') {
        return None;
    }
    Some((host, service))
}

/// Split a canonical container key `host:container_id` and return just the
/// host segment. Rejects an empty host or an empty container-id segment
/// (including keys with no colon at all), mirroring the shape validation
/// `split_service_instance_key` applies to service-instance keys.
///
/// This validates *shape*, not canonicality: the returned host is not
/// checked against the canonical character set, so do not use this as an
/// input validator for untrusted keys.
pub fn container_key_host(key: &str) -> Option<&str> {
    let (host, container) = key.split_once(':')?;
    if host.is_empty() || container.is_empty() {
        return None;
    }
    Some(host)
}

/// Classify legacy service identity shapes (`nashost:plex`,
/// `nashost:plex:plex`, `plex/plex/plex`). Canonical inputs return `None`, as
/// do free-text inputs that merely contain colons or slashes without looking
/// like legacy keys: anything with ASCII whitespace, colon shapes whose
/// segments are not all name-like (`10.0.0.5:443`, `12:30`) or contain a
/// slash (URLs like `http://example.com`, URIs like `agent-command://foo`),
/// and absolute paths (`/mnt/user/media`).
pub fn classify_legacy_shape(value: &str) -> Option<LegacyShape> {
    let trimmed = value.trim();
    if trimmed.chars().any(|ch| ch.is_ascii_whitespace()) {
        return None;
    }
    let colon_count = trimmed.matches(':').count();
    if colon_count >= 1 {
        // A colon segment containing `/` means the input is a URL/URI, not a
        // legacy `host:service` key. Returning `None` here (instead of
        // falling through) also keeps `://` strings out of the slash-triplet
        // branch: `https://a.b/c` has two slashes but is never a legacy shape.
        let name_like_segments = trimmed.split(':').all(|segment| {
            !segment.contains('/') && segment.chars().any(|ch| ch.is_ascii_alphabetic())
        });
        if !name_like_segments {
            return None;
        }
        if colon_count == 1 {
            return Some(LegacyShape::HostService);
        }
        return Some(LegacyShape::HostProjectService);
    }
    // NOTE(over-match tradeoff): this matches ANY 2+-slash, non-absolute-path
    // string, not just the specific `{compose_project}/{compose_service}/
    // {container_name}` shape the old `agent::docker::container_app_name`
    // used to emit (that agent path now emits a flat, slash-free APP-NAME —
    // see `agent::docker::container_app_name`'s doc comment). A legitimate
    // 2+-slash app label from another source would still be misclassified
    // as legacy and dropped from graph `app`-entity projection. Investigated
    // as part of syslog-mcp-5k1zb: the only other app-label source in this
    // codebase, OTLP ingest (`otlp::entries::build_entries`), sets `app_name`
    // exclusively from the resource-level `service.name` attribute — never
    // from OTel instrumentation-scope names (e.g.
    // `go.opentelemetry.io/collector/receiver`), which are stored only in
    // `metadata_json.resource_attributes`/`log_attributes`, not `app_name`.
    // So this risk is currently theoretical for OTLP. The legacy central-pull
    // Docker ingest compat path (`docker_ingest::models::ContainerMeta::
    // app_name`, disabled by default, kept for compatibility fixtures/explicit
    // remote Docker Engine endpoints) was also flattened to match the primary
    // agent path (no longer emits a slash-triplet — see that function's doc
    // comment). No currently-active source in this codebase emits a 2+-slash
    // app label; the `SlashTriplet` classification below only matters for
    // historical/already-stored rows and any future producer. If a future
    // app-label source legitimately needs 2+ slashes,
    // narrow this to the specific triplet shape (three non-empty,
    // canonical-component-like segments) instead of widening the exemption
    // list.
    let slash_count = trimmed.matches('/').count();
    if slash_count >= 2 && !trimmed.starts_with('/') {
        return Some(LegacyShape::SlashTriplet);
    }
    None
}

fn canonical_component(value: &str) -> Option<String> {
    let out = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_string();
    (!out.is_empty()).then_some(out)
}
