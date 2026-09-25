use crate::db;

use super::*;

#[test]
fn log_entry_conversion_preserves_network_sender_identity() {
    let entry = LogEntry::from(db::LogEntry {
        id: 42,
        timestamp: "2026-01-01T00:00:00Z".into(),
        hostname: "claimed-host".into(),
        facility: Some("local0".into()),
        severity: "warning".into(),
        app_name: Some("rsyslogd".into()),
        process_id: Some("123".into()),
        message: "message".into(),
        received_at: "2026-01-01T00:00:01Z".into(),
        source_ip: "192.0.2.10:514".into(),
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: None,
    });

    assert_eq!(entry.hostname, "claimed-host");
    assert_eq!(entry.source_ip, "192.0.2.10:514");
    assert_eq!(entry.app_name.as_deref(), Some("rsyslogd"));
}

#[test]
fn summary_and_host_conversions_preserve_counts() {
    let summary = ErrorSummaryEntry::from(db::ErrorSummaryEntry {
        hostname: "host-a".into(),
        app_name: None,
        severity: "err".into(),
        count: 7,
    });
    let host = HostEntry::from(db::HostEntry {
        hostname: "host-a".into(),
        first_seen: "2026-01-01T00:00:00Z".into(),
        last_seen: "2026-01-01T01:00:00Z".into(),
        log_count: 11,
    });

    assert_eq!(summary.count, 7);
    assert_eq!(host.log_count, 11);
}

#[test]
fn db_stats_conversion_preserves_guardrail_fields() {
    let stats = DbStats::from(db::DbStats {
        total_logs: 10,
        total_hosts: 2,
        oldest_log: Some("2026-01-01T00:00:00Z".into()),
        newest_log: Some("2026-01-02T00:00:00Z".into()),
        logical_db_size_mb: "1.25".into(),
        physical_db_size_mb: "2.50".into(),
        free_disk_mb: Some("512.00".into()),
        max_db_size_mb: 1024,
        min_free_disk_mb: 512,
        write_blocked: true,
        phantom_fts_rows: Some(3),
    });

    assert_eq!(stats.total_logs, 10);
    assert_eq!(stats.free_disk_mb.as_deref(), Some("512.00"));
    assert!(stats.write_blocked);
    assert_eq!(stats.phantom_fts_rows, Some(3));
}

#[test]
fn ai_inventory_conversions_preserve_counts() {
    let tools = ListAiToolsResponse::from(db::ListAiToolsResult {
        total_tools: 1,
        truncated: false,
        tools: vec![db::AiToolInventoryEntry {
            tool: "claude".into(),
            event_count: 4,
            session_count: 2,
            first_seen: "2026-01-01T00:00:00Z".into(),
            last_seen: "2026-01-01T01:00:00Z".into(),
        }],
    });
    let projects = ListAiProjectsResponse::from(db::ListAiProjectsResult {
        total_projects: 1,
        truncated: false,
        projects: vec![db::AiProjectInventoryEntry {
            project: "/tmp/project".into(),
            tools: vec!["claude".into()],
            event_count: 4,
            session_count: 2,
            first_seen: "2026-01-01T00:00:00Z".into(),
            last_seen: "2026-01-01T01:00:00Z".into(),
        }],
    });

    assert_eq!(tools.tools[0].tool, "claude");
    assert_eq!(tools.total_tools, 1);
    assert!(!tools.truncated);
    assert_eq!(projects.projects[0].project, "/tmp/project");
    assert_eq!(projects.total_projects, 1);
    assert!(!projects.truncated);
}

#[test]
fn request_actor_prefers_verified_email_for_display() {
    let actor = RequestActor::mcp_identity(Some("sub-123".into()), Some("me@example.com".into()));

    assert_eq!(actor.surface, "mcp");
    assert_eq!(actor.display, "me@example.com");
    assert_eq!(actor.subject.as_deref(), Some("sub-123"));
    assert_eq!(actor.email.as_deref(), Some("me@example.com"));
}

#[test]
fn ai_correlate_rest_policy_clamps_and_reports_effective_limit() {
    let (req, clamped_to) = AiCorrelateRequest {
        events_per_anchor: Some(10_000),
        ..Default::default()
    }
    .normalize_limits(AiCorrelateLimitPolicy::REST);

    assert_eq!(req.events_per_anchor, Some(50));
    assert_eq!(clamped_to, Some(50));
}

#[test]
fn notifications_recent_request_owns_default_and_limit_clamp() {
    assert_eq!(
        NotificationsRecentRequest {
            limit: None,
            rule_id: None,
            hostname: None,
            since: None,
            until: None,
        }
        .effective_limit(),
        50
    );
    assert_eq!(
        NotificationsRecentRequest {
            limit: Some(10_000),
            rule_id: None,
            hostname: None,
            since: None,
            until: None,
        }
        .effective_limit(),
        500
    );
}

#[test]
fn db_checkpoint_request_validates_allowed_modes() {
    assert_eq!(
        DbCheckpointRequest {
            mode: "FULL".into()
        }
        .normalized_mode()
        .unwrap(),
        "full"
    );

    let err = DbCheckpointRequest {
        mode: "evil".into(),
    }
    .normalized_mode()
    .unwrap_err();
    assert!(err.to_string().contains("mode must be one of"));
}
