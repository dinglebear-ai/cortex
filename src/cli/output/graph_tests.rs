use super::*;

fn graph_metadata() -> cortex::app::GraphResponseMetadata {
    cortex::app::GraphResponseMetadata {
        truncated: false,
        truncated_reason: None,
        limit: 20,
        depth: 1,
        evidence_sample_limit: 2,
        payload_budget: 32_768,
        projection_status: "ready".into(),
        last_completed_at: None,
        source_watermark: "logs:1;heartbeats:0;signatures:0".into(),
        last_error: None,
        is_degraded: false,
    }
}

fn graph_status() -> cortex::app::GraphProjectionStatusResponse {
    cortex::app::GraphProjectionStatusResponse {
        projection_status: "ready".into(),
        last_started_at: Some("2026-06-13T00:00:00Z".into()),
        last_completed_at: Some("2026-06-13T00:00:01Z".into()),
        source_watermark: "logs:1".into(),
        source_row_count: 10,
        entity_count: 2,
        relationship_count: 1,
        evidence_count: 1,
        is_degraded: true,
        last_error: Some("previous failure".into()),
        last_runtime_ms: 15,
        last_chunk_count: 1,
    }
}

fn graph_entity(id: i64, entity_type: &str, key: &str) -> cortex::app::GraphEntity {
    cortex::app::GraphEntity {
        id,
        entity_type: entity_type.into(),
        canonical_key: key.into(),
        display_label: key.into(),
        source_kind: "log".into(),
        source_id: format!("log:{id}"),
        trust_level: "verified".into(),
        first_seen_at: Some("2026-06-13T00:00:00Z".into()),
        last_seen_at: Some("2026-06-13T00:01:00Z".into()),
    }
}

fn graph_summary(id: i64, entity_type: &str, key: &str) -> cortex::app::GraphEntitySummary {
    cortex::app::GraphEntitySummary {
        id,
        entity_type: entity_type.into(),
        canonical_key: key.into(),
        display_label: key.into(),
        trust_level: "verified".into(),
    }
}

fn graph_relationship() -> cortex::app::GraphRelationship {
    cortex::app::GraphRelationship {
        id: 7,
        relationship_key: "app:sshd->host:host-a".into(),
        src_entity_id: 1,
        dst_entity_id: 2,
        src_entity: Some(graph_summary(1, "app", "sshd")),
        dst_entity: Some(graph_summary(2, "host", "host-a")),
        relationship_type: "emitted_by".into(),
        reason_code: "log_app_name".into(),
        trust_level: "verified".into(),
        confidence: 0.9,
        evidence_count: 1,
        evidence_ids: vec![9],
        first_seen_at: Some("2026-06-13T00:00:00Z".into()),
        last_seen_at: Some("2026-06-13T00:01:00Z".into()),
    }
}

fn graph_evidence() -> cortex::app::GraphEvidence {
    cortex::app::GraphEvidence {
        id: 9,
        relationship_id: 7,
        source_kind: "log".into(),
        source_id: "log:1".into(),
        source_log_id: Some(1),
        source_heartbeat_id: None,
        source_signature_hash: None,
        observed_at: "2026-06-13T00:00:00Z".into(),
        reason_code: "log_app_name".into(),
        reason_text: Some("app emitted log".into()),
        confidence_delta: 0.1,
        trust_level: "verified".into(),
        safe_excerpt: Some("accepted connection".into()),
        metadata_path: Some("$.app_name".into()),
        evidence_count: 1,
    }
}

#[test]
fn graph_safe_display_escapes_terminal_control_characters() {
    assert_eq!(safe_display("host\x1b[31m\nnext"), "host\\u{1b}[31m\\nnext");
}

#[test]
fn graph_status_and_rebuild_human_outputs_accept_degraded_payloads() {
    print_graph_status_response(&graph_status(), false).unwrap();
    print_graph_rebuild_response(
        &cortex::app::GraphRebuildResponse {
            outcome: "rebuilt".into(),
            stats: Some(cortex::app::GraphRebuildStatsResponse {
                source_row_count: 10,
                entity_count: 2,
                relationship_count: 1,
                evidence_count: 1,
                source_watermark: "logs:1".into(),
                runtime_ms: 15,
                chunk_count: 1,
            }),
            status: graph_status(),
        },
        false,
    )
    .unwrap();
}

#[test]
fn graph_lookup_around_and_explain_human_outputs_accept_resolved_and_ambiguous_payloads() {
    let entity = graph_entity(1, "app", "sshd");
    let candidate = cortex::app::GraphEntityCandidate {
        entity: graph_entity(3, "app", "ssh"),
        match_reason: "alias".into(),
        alias_type: Some("app_name".into()),
        alias_key: Some("ssh".into()),
    };

    print_graph_entity_lookup_response(
        &GraphEntityLookupResponse {
            resolved_entity: Some(entity.clone()),
            candidates: vec![candidate.clone()],
            metadata: graph_metadata(),
        },
        false,
    )
    .unwrap();

    print_graph_around_response(
        &GraphAroundResponse {
            resolved_entity: Some(entity.clone()),
            entities: vec![entity.clone(), graph_entity(2, "host", "host-a")],
            relationships: vec![graph_relationship()],
            evidence: vec![graph_evidence()],
            next_queries: vec![cortex::app::GraphNextQuery {
                mode: "around".into(),
                entity_id: 2,
                label: "host-a".into(),
            }],
            candidates: Vec::new(),
            metadata: graph_metadata(),
        },
        false,
    )
    .unwrap();

    print_graph_around_response(
        &GraphAroundResponse {
            resolved_entity: None,
            entities: Vec::new(),
            relationships: Vec::new(),
            evidence: Vec::new(),
            next_queries: Vec::new(),
            candidates: vec![candidate],
            metadata: graph_metadata(),
        },
        false,
    )
    .unwrap();

    print_graph_explain_response(
        &GraphExplainResponse {
            resolved_entity: Some(entity),
            narrative: Some(cortex::app::GraphIncidentNarrative {
                title: "App emits logs".into(),
                summary: "sshd emitted on host-a".into(),
                confidence: "high".into(),
                relationship_ids: vec![7],
                evidence_ids: vec![9],
            }),
            chains: vec![cortex::app::GraphNarrativeChain {
                chain_id: "chain-1".into(),
                confidence: "high".into(),
                score: 0.9,
                summary: "relationship is backed by log evidence".into(),
                entities: Vec::new(),
                relationships: vec![graph_relationship()],
                evidence_ids: vec![9],
                relationship_ids: vec![7],
                open_questions: vec!["what changed?".into()],
            }],
            evidence: vec![graph_evidence()],
            open_questions: vec!["what changed?".into()],
            missing_evidence: vec!["heartbeat".into()],
            next_queries: vec![cortex::app::GraphNextQuery {
                mode: "around".into(),
                entity_id: 1,
                label: "sshd".into(),
            }],
            candidates: Vec::new(),
            metadata: graph_metadata(),
        },
        false,
    )
    .unwrap();
}

#[test]
fn graph_entity_json_output_accepts_empty_candidate_response() {
    let response = GraphEntityLookupResponse {
        resolved_entity: None,
        candidates: Vec::new(),
        metadata: graph_metadata(),
    };

    print_graph_entity_lookup_response(&response, true).unwrap();
}

#[test]
fn graph_explain_json_output_accepts_empty_response() {
    let response = GraphExplainResponse {
        resolved_entity: None,
        narrative: None,
        chains: Vec::new(),
        evidence: Vec::new(),
        open_questions: vec!["what changed?".into()],
        missing_evidence: vec!["relationship evidence".into()],
        next_queries: Vec::new(),
        candidates: Vec::new(),
        metadata: graph_metadata(),
    };

    print_graph_explain_response(&response, true).unwrap();
}

#[test]
fn graph_evidence_json_output_accepts_safe_response() {
    let src = cortex::app::GraphEntitySummary {
        id: 1,
        entity_type: "app".into(),
        canonical_key: "sshd".into(),
        display_label: "sshd".into(),
        trust_level: "verified".into(),
    };
    let dst = cortex::app::GraphEntitySummary {
        id: 2,
        entity_type: "host".into(),
        canonical_key: "host-a".into(),
        display_label: "host-a".into(),
        trust_level: "claimed".into(),
    };
    let response = cortex::app::GraphEvidenceLookupResponse {
        evidence: cortex::app::GraphEvidence {
            id: 9,
            relationship_id: 7,
            source_kind: "log".into(),
            source_id: "log:1".into(),
            source_log_id: Some(1),
            source_heartbeat_id: None,
            source_signature_hash: None,
            observed_at: "2026-01-01T00:00:00Z".into(),
            reason_code: "log_app_name".into(),
            reason_text: Some("app emitted log".into()),
            confidence_delta: 0.1,
            trust_level: "verified".into(),
            safe_excerpt: Some("accepted connection".into()),
            metadata_path: Some("$.app_name".into()),
            evidence_count: 1,
        },
        relationship: cortex::app::GraphRelationship {
            id: 7,
            relationship_key: "app:sshd->host:host-a".into(),
            src_entity_id: 1,
            dst_entity_id: 2,
            src_entity: Some(src.clone()),
            dst_entity: Some(dst.clone()),
            relationship_type: "emitted_by".into(),
            reason_code: "log_app_name".into(),
            trust_level: "verified".into(),
            confidence: 0.9,
            evidence_count: 1,
            evidence_ids: vec![9],
            first_seen_at: None,
            last_seen_at: None,
        },
        src_entity: src,
        dst_entity: dst,
        source_log_summary: Some(cortex::app::GraphSourceLogSummary {
            id: 1,
            timestamp: "2026-01-01T00:00:00Z".into(),
            received_at: "2026-01-01T00:00:00Z".into(),
            hostname: "host-a".into(),
            severity: "info".into(),
            app_name: Some("sshd".into()),
            process_id: None,
            source_ip: "10.0.0.1:514".into(),
            message: "line with \\u{1b} escaped".into(),
            message_truncated: false,
        }),
        missing_source_reason: None,
        metadata: graph_metadata(),
    };

    print_graph_evidence_lookup_response(&response, true).unwrap();
    print_graph_evidence_lookup_response(&response, false).unwrap();
}

#[test]
fn graph_evidence_human_output_reports_missing_source_reason() {
    let response = cortex::app::GraphEvidenceLookupResponse {
        evidence: graph_evidence(),
        relationship: graph_relationship(),
        src_entity: graph_summary(1, "app", "sshd"),
        dst_entity: graph_summary(2, "host", "host-a"),
        source_log_summary: None,
        missing_source_reason: Some("source log deleted".into()),
        metadata: graph_metadata(),
    };

    print_graph_evidence_lookup_response(&response, false).unwrap();
}

#[test]
fn graph_output_formats_resolver_entity_types_as_first_class_rows() {
    // logical_service / service_instance rows format through the same
    // entity pipeline as every other entity type — typed label first, no
    // generic unknown-type fallback.
    let logical = entity_line("entity", &graph_entity(1, "logical_service", "plex"));
    assert!(logical.contains("logical_service"));
    assert!(logical.contains("plex"));

    let instance = entity_line(
        "entity",
        &graph_entity(2, "service_instance", "nashost/plex"),
    );
    assert!(instance.contains("service_instance"));
    assert!(instance.contains("nashost/plex"));

    let summary = entity_summary_label(&graph_summary(3, "service_instance", "nashost/plex"));
    assert!(summary.contains("service_instance"));
    assert!(summary.contains("nashost/plex"));
}
