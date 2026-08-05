//! Tests for the graph-anchored, session-scoped lane of `ai_correlate`
//! (`correlate_session_graph` shaping + the `ai_correlate` end-to-end path).

use super::*;
use crate::config::StorageConfig;
use crate::db::{LogBatchEntry, init_pool, insert_logs_batch};
use std::sync::Arc;

fn test_service() -> (CortexService, Arc<crate::db::DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("ai-correlate-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

fn db_log(source_ip: &str, host: &str, source_kind: &str) -> db::LogEntry {
    db::LogEntry {
        id: 1,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        hostname: host.to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some("app".to_string()),
        process_id: None,
        message: "m".to_string(),
        received_at: "2026-01-01T00:00:00Z".to_string(),
        source_ip: source_ip.to_string(),
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: Some(format!(r#"{{"source_kind":"{source_kind}"}}"#)),
    }
}

fn heartbeat(host: &str) -> db::HeartbeatWindowSummary {
    db::HeartbeatWindowSummary {
        host_id: format!("hb-{host}"),
        hostname: host.to_string(),
        samples: 3,
        partial_samples: 0,
        max_cpu_usage_percent: Some(50.0),
        min_mem_available_bytes: Some(1024),
        pressure_flags: Vec::new(),
    }
}

#[test]
fn row_source_kind_parses_metadata() {
    let row = db_log("10.0.0.1:514", "devhost", "syslog-udp");
    assert_eq!(row_source_kind(&row).as_deref(), Some("syslog-udp"));

    let mut no_meta = row.clone();
    no_meta.metadata_json = None;
    assert_eq!(row_source_kind(&no_meta), None);
}

#[test]
fn build_graph_session_correlation_classifies_lanes_and_filters_heartbeats() {
    let inputs = db::SessionGraphInputs {
        bounds: Some(("2026-01-01T00:00:00Z".into(), "2026-01-01T00:10:00Z".into())),
        discovered_hosts: vec!["devhost".into()],
        discovered_entities: vec!["devhost".into(), "cortex".into()],
        used_graph: true,
        logs: vec![
            db_log(
                "agent-command://devhost/claude/s1",
                "devhost",
                "agent-command",
            ),
            db_log("10.0.0.9:0", "devhost", "shell-history"),
            db_log("10.0.0.5:514", "devhost", "syslog-udp"),
        ],
    };
    // One summary for a discovered host, one for an unrelated host (filtered out).
    let summaries = vec![heartbeat("devhost"), heartbeat("edgehost")];

    let out = build_graph_session_correlation("s1".into(), inputs, summaries).unwrap();
    assert!(out.used_graph);
    assert_eq!(out.session_start, "2026-01-01T00:00:00Z");
    assert_eq!(out.agent_command_count, 1);
    assert_eq!(out.shell_history_count, 1);
    assert_eq!(out.discovered_hosts, vec!["devhost"]);

    let discoveries: Vec<&str> = out.logs.iter().map(|l| l.discovery.as_str()).collect();
    assert!(discoveries.contains(&"agent_command"));
    assert!(discoveries.contains(&"shell_history"));
    assert!(discoveries.iter().any(|d| d.starts_with("graph:host:")));

    // Heartbeats filtered to discovered hosts only.
    assert_eq!(out.heartbeat_summaries.len(), 1);
    assert_eq!(out.heartbeat_summaries[0].hostname, "devhost");
}

#[test]
fn build_graph_session_correlation_none_for_empty_bounds() {
    let inputs = db::SessionGraphInputs::default();
    assert!(build_graph_session_correlation("s1".into(), inputs, Vec::new()).is_none());
}

fn agent_command_log(ts: &str, host: &str, session: &str, cwd: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: Some("agent".to_string()),
        severity: "info".to_string(),
        app_name: Some("claude".to_string()),
        process_id: None,
        message: "cargo test".to_string(),
        raw: "cargo test".to_string(),
        source_ip: format!("agent-command://{host}/claude/{session}"),
        docker_checkpoint: None,
        ai_tool: Some("claude".to_string()),
        ai_project: Some(cwd.to_string()),
        ai_session_id: Some(session.to_string()),
        ai_transcript_path: None,
        metadata_json: Some(format!(
            r#"{{"source_kind":"agent-command","agent_command":{{"cwd":"{cwd}"}}}}"#
        )),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: Some("command".to_string()),
        parse_error: None,
    }
}

fn plain_syslog(ts: &str, host: &str, app: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: None,
        severity: "warning".to_string(),
        app_name: Some(app.to_string()),
        process_id: None,
        message: format!("{app} broke"),
        raw: format!("{app} broke"),
        source_ip: "10.0.0.5:514".to_string(),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: Some(r#"{"source_kind":"syslog-udp"}"#.to_string()),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn ai_correlate_session_uses_graph_and_discovers_hosts() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, pool, _dir) = test_service();
    insert_logs_batch(
        &pool,
        &[
            agent_command_log(
                "2026-01-01T00:00:00Z",
                "devhost",
                "sess-7",
                "/home/jmagar/workspace/cortex",
            ),
            plain_syslog("2026-01-01T00:01:00Z", "devhost", "swag"),
            plain_syslog("2026-01-01T00:02:00Z", "edgehost", "authelia"),
        ],
    )
    .unwrap();
    crate::db::graph::refresh_graph_projection(&pool).unwrap();

    let req = AiCorrelateRequest {
        session_id: Some("sess-7".to_string()),
        ..Default::default()
    };
    let resp = svc.correlate_ai_logs(req).await.unwrap();

    let gc = resp
        .graph_correlation
        .expect("session-targeted correlation must populate graph_correlation");
    assert!(
        gc.used_graph,
        "session is projected, graph lane must be used"
    );
    assert_eq!(gc.session_id, "sess-7");
    assert!(
        gc.discovered_hosts.contains(&"devhost".to_string()),
        "host discovered via session→host edge: {:?}",
        gc.discovered_hosts
    );
    assert!(
        gc.agent_command_count >= 1,
        "Claude's bash call must appear in the agent-command lane"
    );
    let hosts: std::collections::HashSet<&str> =
        gc.logs.iter().map(|l| l.entry.hostname.as_str()).collect();
    assert!(hosts.contains("devhost"));
    assert!(!hosts.contains("edgehost"), "unrelated host excluded");
}

#[tokio::test]
async fn ai_correlate_session_falls_back_when_graph_unprojected() {
    let (svc, pool, _dir) = test_service();
    // Insert a session row but do NOT refresh the graph → no ai_session entity.
    insert_logs_batch(
        &pool,
        &[agent_command_log(
            "2026-01-01T00:00:00Z",
            "devhost",
            "sess-9",
            "/home/jmagar/workspace/cortex",
        )],
    )
    .unwrap();

    let req = AiCorrelateRequest {
        session_id: Some("sess-9".to_string()),
        ..Default::default()
    };
    let resp = svc.correlate_ai_logs(req).await.unwrap();
    let gc = resp
        .graph_correlation
        .expect("bounds exist → block present");
    assert!(!gc.used_graph, "no graph entity yet → fallback mode");
    assert!(
        !gc.logs.is_empty(),
        "fallback still returns the session's own rows"
    );
}

#[tokio::test]
async fn ai_correlate_without_session_omits_graph_lane() {
    let (svc, pool, _dir) = test_service();
    insert_logs_batch(
        &pool,
        &[plain_syslog("2026-01-01T00:00:00Z", "devhost", "swag")],
    )
    .unwrap();

    let req = AiCorrelateRequest {
        project: Some("cortex".to_string()),
        ..Default::default()
    };
    let resp = svc.correlate_ai_logs(req).await.unwrap();
    assert!(
        resp.graph_correlation.is_none(),
        "graph lane is session-anchored; absent without a session_id"
    );
}
