//! Tests for the `topic_correlate` graph-anchored universal correlation action.

use super::*;
use crate::config::StorageConfig;
use crate::db::{LogBatchEntry, init_pool, insert_logs_batch};
use std::sync::Arc;

fn test_service() -> (CortexService, Arc<crate::db::DbPool>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("topic-correlate-test.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    (CortexService::new(Arc::clone(&pool), storage), pool, dir)
}

fn agent_command_log(ts: &str, host: &str, session: &str, cwd: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: Some("agent".to_string()),
        severity: "info".to_string(),
        app_name: Some("claude".to_string()),
        process_id: None,
        message: "cargo build --release --bin axon".to_string(),
        raw: "cargo build".to_string(),
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

fn syslog(ts: &str, host: &str, app: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: host.to_string(),
        facility: None,
        severity: "warning".to_string(),
        app_name: Some(app.to_string()),
        process_id: None,
        message: format!("{app} event"),
        raw: format!("{app} event"),
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

async fn seeded_service() -> (CortexService, Arc<crate::db::DbPool>, tempfile::TempDir) {
    let (svc, pool, dir) = test_service();
    insert_logs_batch(
        &pool,
        &[
            agent_command_log(
                "2026-01-01T00:00:00Z",
                "devhost",
                "sess-7",
                "/home/jmagar/workspace/axon",
            ),
            syslog("2026-01-01T00:01:00Z", "devhost", "swag"),
            syslog("2026-01-01T00:02:00Z", "edgehost", "authelia"),
        ],
    )
    .unwrap();
    crate::db::graph::refresh_graph_projection(&pool).unwrap();
    (svc, pool, dir)
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_resolves_project_and_builds_timeline() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, _pool, _dir) = seeded_service().await;

    let req = TopicCorrelateRequest {
        topic: "axon".to_string(),
        since: Some("2026-01-01T00:00:00Z".into()),
        until: Some("2026-01-01T01:00:00Z".into()),
        ..Default::default()
    };
    let resp = svc.topic_correlate(req).await.unwrap();

    assert_eq!(resp.topic, "axon");
    assert!(
        resp.resolved_entities
            .iter()
            .any(|e| e.entity_type == "ai_project" && e.key == "axon"),
        "topic must resolve to ai_project:axon: {:?}",
        resp.resolved_entities
    );
    // Graph expansion reaches the host the session ran on.
    assert!(
        resp.discovered_hosts.contains(&"devhost".to_string()),
        "expansion must discover devhost: {:?}",
        resp.discovered_hosts
    );
    // Timeline carries the agent-command row (Claude worked on axon).
    assert!(
        resp.timeline
            .iter()
            .any(|t| t.entity_path == "agent_command"),
        "timeline must include the agent-command lane"
    );
    // Unrelated host's logs are not pulled in.
    assert!(resp.timeline.iter().all(|t| t.hostname != "edgehost"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_with_no_match_is_empty() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, _pool, _dir) = seeded_service().await;

    let req = TopicCorrelateRequest {
        topic: "nonexistent-topic-xyz".to_string(),
        ..Default::default()
    };
    let resp = svc.topic_correlate(req).await.unwrap();
    assert!(resp.resolved_entities.is_empty());
    assert!(resp.timeline.is_empty());
    assert!(!resp.truncated);
}

#[tokio::test]
async fn topic_resolves_ai_project_without_graph_projection() {
    let (svc, pool, _dir) = test_service();
    insert_logs_batch(
        &pool,
        &[agent_command_log(
            "2026-01-01T00:00:00Z",
            "devhost",
            "sess-7",
            "/home/jmagar/workspace/ci-runner-farm",
        )],
    )
    .unwrap();

    let resp = svc
        .topic_correlate(TopicCorrelateRequest {
            topic: "ci-runner-farm".into(),
            since: Some("2026-01-01T00:00:00Z".into()),
            until: Some("2026-01-01T01:00:00Z".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(resp.resolved_entities.iter().any(|entity| {
        entity.entity_type == "ai_project"
            && entity.key == "ci-runner-farm"
            && entity.resolver_status.as_deref() == Some("degraded")
    }));
    assert_eq!(resp.timeline.len(), 1);
    assert_eq!(
        resp.timeline[0].fallback_kind.as_deref(),
        Some("direct_source_identity")
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_multi_term_resolves_host_and_project() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, _pool, _dir) = seeded_service().await;

    let req = TopicCorrelateRequest {
        topic: "devhost axon".to_string(),
        ..Default::default()
    };
    let resp = svc.topic_correlate(req).await.unwrap();
    let types: std::collections::HashSet<&str> = resp
        .resolved_entities
        .iter()
        .map(|e| e.entity_type.as_str())
        .collect();
    assert!(types.contains("host"), "devhost → host: {types:?}");
    assert!(types.contains("ai_project"), "axon → ai_project: {types:?}");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_source_kind_filter_restricts_timeline() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, _pool, _dir) = seeded_service().await;

    let req = TopicCorrelateRequest {
        topic: "devhost".to_string(),
        since: Some("2026-01-01T00:00:00Z".into()),
        until: Some("2026-01-01T01:00:00Z".into()),
        source_kinds: Some(vec!["syslog-udp".to_string()]),
        ..Default::default()
    };
    let resp = svc.topic_correlate(req).await.unwrap();
    assert!(!resp.timeline.is_empty());
    assert!(
        resp.timeline
            .iter()
            .all(|t| t.entity_path != "agent_command"),
        "agent-command rows excluded by the syslog-udp filter"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_source_kind_accepts_string_form() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, _pool, _dir) = seeded_service().await;

    let req: TopicCorrelateRequest = serde_json::from_value(serde_json::json!({
        "topic": "devhost",
        "since": "2026-01-01T00:00:00Z",
        "until": "2026-01-01T01:00:00Z",
        "source_kinds": "syslog-udp"
    }))
    .unwrap();
    let resp = svc.topic_correlate(req).await.unwrap();

    assert!(!resp.timeline.is_empty());
    assert!(
        resp.timeline
            .iter()
            .all(|t| t.entity_path != "agent_command"),
        "string-form source_kinds must apply the same filter as an array"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_source_kind_rejects_invalid_filter() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, _pool, _dir) = seeded_service().await;

    let req = TopicCorrelateRequest {
        topic: "devhost".to_string(),
        source_kinds: Some(vec!["syslog".to_string()]),
        ..Default::default()
    };
    let err = svc.topic_correlate(req).await.unwrap_err();

    assert!(matches!(err, crate::app::ServiceError::InvalidInput(_)));
    assert!(err.to_string().contains("invalid source_kinds"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_source_kind_rejects_request_with_any_invalid_kind() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, _pool, _dir) = seeded_service().await;

    // A single bad value rejects the whole request (no silent dropping), and the
    // error names every invalid kind.
    let req = TopicCorrelateRequest {
        topic: "devhost".to_string(),
        source_kinds: Some(vec![
            "syslog-udp".to_string(),
            "nope".to_string(),
            "also-bad".to_string(),
        ]),
        ..Default::default()
    };
    let err = svc.topic_correlate(req).await.unwrap_err();
    assert!(matches!(err, crate::app::ServiceError::InvalidInput(_)));
    let msg = err.to_string();
    assert!(
        msg.contains("nope") && msg.contains("also-bad"),
        "error must list all invalid kinds: {msg}"
    );
}

#[test]
fn source_kinds_deserializer_rejects_non_string_values() {
    // A non-string array element is a hard error, not a silent coercion.
    let err = serde_json::from_value::<TopicCorrelateRequest>(serde_json::json!({
        "topic": "x",
        "source_kinds": ["syslog-udp", 5]
    }))
    .unwrap_err();
    assert!(err.to_string().contains("source_kinds"), "{err}");

    // A non-string, non-array scalar is rejected too.
    let err = serde_json::from_value::<TopicCorrelateRequest>(serde_json::json!({
        "topic": "x",
        "source_kinds": 5
    }))
    .unwrap_err();
    assert!(err.to_string().contains("source_kinds"), "{err}");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_plex_uses_service_instance_without_host_wide_fanout() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, pool, _dir) = test_service();
    let mut plex = syslog("2026-01-01T00:01:00Z", "nashost", "plex/plex/plex");
    plex.message = "Plex library scan".to_string();
    plex.raw = "Plex library scan".to_string();
    plex.metadata_json = Some(
        r#"{"source_kind":"agent-docker","agent_docker":{"host":"nashost","container_id":"abcdef1234567890","container_name":"plex","compose_project":"plex","compose_service":"plex","stream":"stdout"}}"#
            .to_string(),
    );
    insert_logs_batch(
        &pool,
        &[syslog("2026-01-01T00:00:00Z", "nashost", "kernel"), plex],
    )
    .unwrap();
    crate::db::graph::refresh_graph_projection(&pool).unwrap();
    let resp = svc
        .topic_correlate(TopicCorrelateRequest {
            topic: "plex".to_string(),
            since: Some("2026-01-01T00:00:00Z".into()),
            until: Some("2026-01-01T01:00:00Z".into()),
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        resp.resolved_entities
            .iter()
            .any(|e| { e.entity_type == "logical_service" && e.key == "plex" }),
        "topic must resolve logical_service:plex: {:?}",
        resp.resolved_entities
    );
    assert!(
        resp.timeline
            .iter()
            .any(|row| row.message.contains("Plex library scan"))
    );
    // No silent host-wide fan-out: the kernel row on nashost stays out.
    assert!(
        !resp
            .timeline
            .iter()
            .any(|row| row.app_name.as_deref() == Some("kernel")),
        "topic plex must not fan out to all nashost logs: {:?}",
        resp.timeline
    );
    assert!(resp.timeline.iter().all(|row| {
        row.inclusion_reason.as_deref() == Some("service_instance")
            || row.fallback_kind.as_deref() == Some("explicit_degraded_host_context")
    }));
}

#[tokio::test]
async fn topic_rejects_legacy_service_shapes() {
    let (svc, _pool, _dir) = test_service();
    for topic in ["nashost:plex", "nashost:plex:plex", "plex/plex/plex"] {
        let err = svc
            .topic_correlate(TopicCorrelateRequest {
                topic: topic.to_string(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rejected_legacy_shape"), "{topic}");
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_service_instance_fallback_to_host_context_is_explicit() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, pool, _dir) = test_service();
    // The plex service instance exists in the graph (via agent-docker
    // identity), but the only logs in the window are host-context rows that
    // no service-instance predicate matches.
    let mut plex = syslog("2026-01-01T00:01:00Z", "nashost", "plex/plex/plex");
    plex.metadata_json = Some(
        r#"{"source_kind":"agent-docker","agent_docker":{"host":"nashost","container_id":"abcdef1234567890","container_name":"plex","compose_project":"plex","compose_service":"plex","stream":"stdout"}}"#
            .to_string(),
    );
    insert_logs_batch(
        &pool,
        &[syslog("2026-01-01T00:00:00Z", "nashost", "kernel"), plex],
    )
    .unwrap();
    crate::db::graph::refresh_graph_projection(&pool).unwrap();
    // Restrict the window so the plex row is excluded; only kernel remains.
    let resp = svc
        .topic_correlate(TopicCorrelateRequest {
            topic: "plex".to_string(),
            until: Some("2026-01-01T00:00:30Z".to_string()),
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap();
    // Host-context fallback rows are explicitly annotated, never silent.
    for row in &resp.timeline {
        assert_eq!(
            row.fallback_kind.as_deref(),
            Some("explicit_degraded_host_context"),
            "fallback rows must be annotated: {row:?}"
        );
        assert_eq!(row.resolver_status.as_deref(), Some("degraded"));
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn topic_defaults_since_to_one_hour_before_until() {
    let _guard = crate::db::graph::GRAPH_TEST_LOCK.lock();
    let (svc, pool, _dir) = test_service();
    let mut older = syslog("2026-01-01T00:30:00Z", "devhost", "cortex");
    older.message = "outside default window".into();
    older.raw = older.message.clone();
    let mut recent = syslog("2026-01-01T01:30:00Z", "devhost", "cortex");
    recent.message = "inside default window".into();
    recent.raw = recent.message.clone();
    insert_logs_batch(&pool, &[older, recent]).unwrap();
    crate::db::graph::refresh_graph_projection(&pool).unwrap();

    let response = svc
        .topic_correlate(TopicCorrelateRequest {
            topic: "devhost".into(),
            until: Some("2026-01-01T02:00:00Z".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(
        response
            .timeline
            .iter()
            .any(|row| row.message == "inside default window")
    );
    assert!(
        response
            .timeline
            .iter()
            .all(|row| row.message != "outside default window")
    );
}
