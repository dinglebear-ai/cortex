use super::*;

fn minimal_envelope() -> SessionEnvelope {
    let session = AiSessionEntry {
        session_key: "h:codex:p:s".into(),
        project: "p".into(),
        tool: "codex".into(),
        session_id: "s".into(),
        hostname: "h".into(),
        transcript_path: None,
        first_seen: "2026-09-21T00:00:00Z".into(),
        last_seen: "2026-09-21T00:10:00Z".into(),
        event_count: 1,
        title: None,
        title_provenance: None,
    };
    SessionEnvelope {
        metadata: super::super::session_investigation_support::session_investigation_metadata(
            &InvestigationBudget::default(),
            std::time::Instant::now(),
            None,
            false,
            &[],
            1,
            0,
        ),
        result: models::SessionInvestigateResponse {
            session,
            transcript: vec![],
            transcript_has_more: false,
            correlation: None,
            graph_neighborhood: None,
            skill_events: vec![],
            skill_events_truncated: false,
            mcp_events: vec![],
            mcp_events_truncated: false,
            hook_events: vec![],
            hook_events_truncated: false,
            artifact_evidence: vec![],
            artifact_evidence_truncated: false,
            observatory: Default::default(),
            incident_context: IncidentContextResponse {
                window_from: "2026-09-21T00:00:00Z".into(),
                window_to: "2026-09-21T00:10:00Z".into(),
                total_logs: 0,
                by_severity: vec![],
                by_app: vec![],
                error_logs: vec![],
                error_logs_truncated: false,
                ai_sessions: vec![],
            },
            notifications: vec![],
            notifications_truncated: false,
            retention_lineage: vec![],
            retention_lineage_truncated: false,
            related_sessions: vec![],
            related_sessions_truncated: false,
            external_references: vec![],
            external_references_truncated: false,
            source_counts: Default::default(),
            source_evidence: Default::default(),
            partial_reasons: vec![],
        },
    }
}

#[test]
fn escaped_payload_budget_includes_metadata_and_refreshes_counts() {
    let mut envelope = minimal_envelope();
    envelope
        .result
        .transcript
        .push(models::RenderedSessionEvent {
            position: 1,
            timestamp: "2026-09-21T00:00:00Z".into(),
            kind: models::RenderedSessionEventKind::Assistant,
            text: "\"🦀".repeat(40_000),
            redacted: false,
            parse_warning: None,
        });
    envelope.result.source_counts.insert("transcript".into(), 1);
    let result = finalize_session_envelope(envelope).unwrap();
    let size = serde_json::to_vec(&result).unwrap().len();
    assert!(size <= result.metadata.payload_limit_bytes as usize);
    assert_eq!(size, result.metadata.budget_used.payload_bytes as usize);
    assert!(result.metadata.partial && result.metadata.truncated);
    assert!(result.result.transcript_has_more);
    assert_eq!(
        result.result.source_counts["transcript"],
        result.result.transcript.len()
    );
    assert!(
        result
            .result
            .partial_reasons
            .iter()
            .any(|reason| reason == "transcript_payload_truncated")
    );
}

#[test]
fn oversized_required_identity_fails_closed() {
    let mut envelope = minimal_envelope();
    envelope.result.session.project = "p".repeat(100_000);
    assert!(matches!(
        finalize_session_envelope(envelope),
        Err(ServiceError::Busy(_))
    ));
}

#[test]
fn ambiguity_is_partial_without_claiming_truncation() {
    let reasons = vec!["observatory_run_ambiguous".to_string()];
    let metadata = super::super::session_investigation_support::session_investigation_metadata(
        &InvestigationBudget::default(),
        std::time::Instant::now(),
        None,
        false,
        &reasons,
        0,
        0,
    );
    assert!(metadata.partial);
    assert!(!metadata.truncated);
    assert!(metadata.truncation_reasons.is_empty());
    assert_eq!(metadata.auth_state, "unknown");
}

#[tokio::test]
async fn elapsed_budget_returns_retryable_busy() {
    let result: ServiceResult<()> = within_session_budget(1, std::future::pending()).await;
    assert!(
        matches!(result, Err(ServiceError::Busy(message)) if message == "session_investigation_wall_time_budget_exceeded")
    );
}

#[tokio::test]
async fn ready_future_that_overshoots_wall_budget_fails_closed() {
    let result = within_session_budget(1, async {
        std::thread::sleep(std::time::Duration::from_millis(5));
        Ok(())
    })
    .await;
    assert!(matches!(result, Err(ServiceError::Busy(_))));
}
