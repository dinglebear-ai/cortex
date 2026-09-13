use super::*;

pub(super) fn graph_evidence_safe(
    row: db::graph::GraphEvidenceRow,
    payload_budget: u32,
) -> GraphEvidence {
    let excerpt_limit = (payload_budget / 16).clamp(128, 512) as usize;
    GraphEvidence {
        id: row.id,
        relationship_id: row.relationship_id,
        source_kind: row.source_kind,
        source_id: redact_graph_text(row.source_id),
        source_log_id: row.source_log_id,
        source_heartbeat_id: row.source_heartbeat_id,
        source_signature_hash: row.source_signature_hash,
        observed_at: row.observed_at,
        reason_code: row.reason_code,
        reason_text: row.reason_text.map(redact_graph_text),
        confidence_delta: row.confidence_delta,
        trust_level: row.trust_level,
        safe_excerpt: row
            .safe_excerpt
            .map(redact_graph_text)
            .map(|value| truncate_chars(&value, excerpt_limit)),
        metadata_path: row.metadata_path.map(redact_graph_text),
        evidence_count: row.evidence_count,
    }
}

pub(super) fn graph_source_log_summary_safe(
    row: db::graph::GraphSourceLogSummaryRow,
    payload_budget: u32,
) -> GraphSourceLogSummary {
    let message_limit = (payload_budget / 8).clamp(128, 1024) as usize;
    let redacted_message = redact_graph_text_unbounded(row.message);
    let message = truncate_chars(&redacted_message, message_limit);
    GraphSourceLogSummary {
        id: row.id,
        timestamp: redact_graph_text(row.timestamp),
        received_at: redact_graph_text(row.received_at),
        hostname: redact_graph_text(row.hostname),
        severity: redact_graph_text(row.severity),
        app_name: row.app_name.map(redact_graph_text),
        process_id: row.process_id.map(redact_graph_text),
        source_ip: redact_graph_text(row.source_ip),
        message_truncated: redacted_message.chars().count() > message_limit,
        message,
    }
}

/// Redact graph identifiers on a public entity: canonical key, display
/// label, and source id all pass through the same secret/home-path
/// redaction as evidence excerpts. Redaction covers identifiers, not just
/// excerpts — a `/home/...` or token-bearing key must never leak through
/// entity fields.
pub(super) fn graph_entity_safe(entity: GraphEntity) -> GraphEntity {
    GraphEntity {
        canonical_key: redact_graph_text(entity.canonical_key),
        display_label: redact_graph_text(entity.display_label),
        source_id: redact_graph_text(entity.source_id),
        ..entity
    }
}

/// Redact graph identifiers on a lookup candidate (entity + alias key).
pub(super) fn graph_candidate_safe(candidate: GraphEntityCandidate) -> GraphEntityCandidate {
    GraphEntityCandidate {
        entity: graph_entity_safe(candidate.entity),
        match_reason: redact_graph_text(candidate.match_reason),
        alias_type: candidate.alias_type.map(redact_graph_text),
        alias_key: candidate.alias_key.map(redact_graph_text),
    }
}

/// Redact identifiers on a public relationship: the relationship key plus
/// embedded entity summaries.
pub(super) fn graph_relationship_safe(relationship: GraphRelationship) -> GraphRelationship {
    GraphRelationship {
        relationship_key: redact_graph_text(relationship.relationship_key),
        src_entity: relationship.src_entity.map(graph_entity_summary_safe),
        dst_entity: relationship.dst_entity.map(graph_entity_summary_safe),
        ..relationship
    }
}

fn graph_entity_summary_safe(summary: GraphEntitySummary) -> GraphEntitySummary {
    GraphEntitySummary {
        canonical_key: redact_graph_text(summary.canonical_key),
        display_label: redact_graph_text(summary.display_label),
        ..summary
    }
}

pub(super) fn redact_graph_text(value: String) -> String {
    truncate_chars(&redact_graph_text_unbounded(value), 512)
}

fn redact_graph_text_unbounded(value: String) -> String {
    let control_stripped: String = value.chars().filter(|ch| !ch.is_control()).collect();
    let mut out = String::with_capacity(control_stripped.len().min(512));
    let mut redact_next_value = false;
    for token in control_stripped.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        let value_marker = lower.contains("authorization")
            || lower.contains("bearer")
            || lower == "cookie"
            || lower.starts_with("cookie:")
            || lower.contains("set-cookie")
            || lower == "credential"
            || lower == "credentials"
            || lower.starts_with("credential:")
            || lower.starts_with("credentials:");
        let redacted = redact_next_value
            || value_marker
            || lower.starts_with("cookie=")
            || lower.starts_with("credential=")
            || lower.starts_with("credentials=")
            || lower.contains("client_secret")
            || lower.contains("access_token")
            || lower.contains("token=")
            || lower.contains("password")
            || lower.contains("secret")
            || lower.contains("api_key")
            || lower.contains("apikey")
            || lower.contains("private-key")
            || lower.contains("private_key")
            || lower == "begin"
            || lower.contains("-----begin")
            || lower.contains("userinfo")
            || lower.contains("://") && lower.contains('@')
            || lower.contains("/home/")
            || lower.contains("/users/");
        if !out.is_empty() {
            out.push(' ');
        }
        if redacted {
            out.push_str("[redacted]");
        } else {
            out.push_str(token);
        }
        let credential_name = lower.trim_end_matches([':', '=']);
        redact_next_value = (redact_next_value && matches!(token, "=" | ":"))
            || value_marker
            || matches!(
                credential_name,
                "password"
                    | "secret"
                    | "token"
                    | "api_key"
                    | "apikey"
                    | "client_secret"
                    | "access_token"
                    | "private-key"
                    | "private_key"
            );
    }
    out
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

pub(super) fn estimated_graph_payload_bytes(
    entities: &[GraphEntity],
    relationships: &[GraphRelationship],
    evidence: &[GraphEvidence],
) -> usize {
    let entity_bytes: usize = entities
        .iter()
        .map(|entity| {
            entity.display_label.len()
                + entity.canonical_key.len()
                + entity.entity_type.len()
                + entity.trust_level.len()
        })
        .sum();
    let relationship_bytes: usize = relationships
        .iter()
        .map(|rel| {
            rel.relationship_key.len()
                + rel.relationship_type.len()
                + rel.reason_code.len()
                + rel
                    .src_entity
                    .as_ref()
                    .map_or(0, estimated_entity_summary_bytes)
                + rel
                    .dst_entity
                    .as_ref()
                    .map_or(0, estimated_entity_summary_bytes)
        })
        .sum();
    let evidence_bytes: usize = evidence
        .iter()
        .map(|item| {
            item.reason_text.as_ref().map_or(0, String::len)
                + item.safe_excerpt.as_ref().map_or(0, String::len)
                + item.source_id.len()
                + item.reason_code.len()
        })
        .sum();
    entity_bytes + relationship_bytes + evidence_bytes
}

pub(super) fn estimated_graph_evidence_lookup_payload_bytes(
    relationship: &GraphRelationship,
    evidence: &GraphEvidence,
    src_entity: &GraphEntitySummary,
    dst_entity: &GraphEntitySummary,
    source_log_summary: Option<&GraphSourceLogSummary>,
) -> usize {
    let relationship_bytes = relationship.relationship_key.len()
        + relationship.relationship_type.len()
        + relationship.reason_code.len()
        + relationship
            .src_entity
            .as_ref()
            .map_or(0, estimated_entity_summary_bytes)
        + relationship
            .dst_entity
            .as_ref()
            .map_or(0, estimated_entity_summary_bytes);
    let evidence_bytes = evidence.source_id.len()
        + evidence.reason_code.len()
        + evidence.reason_text.as_ref().map_or(0, String::len)
        + evidence.safe_excerpt.as_ref().map_or(0, String::len)
        + evidence.metadata_path.as_ref().map_or(0, String::len);
    let source_bytes = source_log_summary.map_or(0, |summary| {
        summary.timestamp.len()
            + summary.received_at.len()
            + summary.hostname.len()
            + summary.severity.len()
            + summary.app_name.as_ref().map_or(0, String::len)
            + summary.process_id.as_ref().map_or(0, String::len)
            + summary.source_ip.len()
            + summary.message.len()
    });
    let top_level_entity_bytes =
        estimated_entity_summary_bytes(src_entity) + estimated_entity_summary_bytes(dst_entity);
    relationship_bytes + evidence_bytes + top_level_entity_bytes + source_bytes
}

pub(super) fn estimated_entity_summary_bytes(summary: &GraphEntitySummary) -> usize {
    summary.entity_type.len()
        + summary.canonical_key.len()
        + summary.display_label.len()
        + summary.trust_level.len()
}
