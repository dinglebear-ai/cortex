use super::*;

#[test]
fn cursor_round_trips_and_rejects_wrong_contract() {
    assert_eq!(decode_cursor(Some(&encode_cursor(42))).unwrap(), 42);
    assert!(decode_cursor(Some("42")).is_err());
    assert!(decode_cursor(Some("cortex-session-v2:42")).is_err());
    assert!(decode_cursor(Some("cortex-session-v1:-1")).is_err());
}

#[test]
fn projection_preserves_semantics_and_redaction_annotations() {
    let event = project_event(db::RenderedSessionEventRow {
        id: 7,
        timestamp: "2026-08-28T00:00:00Z".into(),
        message: "secret [REDACTED]".into(),
        metadata_json: Some(r#"{"event_kind":"assistant","content_scrubbed":true}"#.into()),
        parse_error: Some("partial record".into()),
    });
    assert_eq!(event.kind, RenderedSessionEventKind::Assistant);
    assert!(event.redacted);
    assert_eq!(event.parse_warning.as_deref(), Some("partial record"));
}

#[test]
fn projection_classifies_tool_summaries_without_raw_payloads() {
    let event = project_event(db::RenderedSessionEventRow {
        id: 8,
        timestamp: "2026-08-28T00:00:00Z".into(),
        message: "[function_call shell]".into(),
        metadata_json: None,
        parse_error: None,
    });
    assert_eq!(event.kind, RenderedSessionEventKind::Tool);
}

#[test]
fn projection_bounds_oversized_utf8_text() {
    let event = project_event(db::RenderedSessionEventRow {
        id: 9,
        timestamp: "2026-08-28T00:00:00Z".into(),
        message: "🦀".repeat(100_000),
        metadata_json: None,
        parse_error: None,
    });
    assert!(event.text.len() <= MAX_EVENT_TEXT_BYTES + "...[truncated]".len());
    assert!(event.text.is_char_boundary(event.text.len()));
    assert_eq!(
        event.parse_warning.as_deref(),
        Some("rendered text truncated")
    );
}

#[test]
fn metadata_and_warning_redaction_make_page_flag_truthful() {
    let metadata_only = project_event(db::RenderedSessionEventRow {
        id: 1,
        timestamp: "2026-08-28T00:00:00Z".into(),
        message: "ordinary message".into(),
        metadata_json: Some(r#"{"nested":"TOKEN=metadata-secret"}"#.into()),
        parse_error: None,
    });
    assert!(metadata_only.redacted);
    let warning_only = project_event(db::RenderedSessionEventRow {
        id: 2,
        timestamp: "2026-08-28T00:00:00Z".into(),
        message: "ordinary message".into(),
        metadata_json: None,
        parse_error: Some("TOKEN=warning-secret".into()),
    });
    assert!(warning_only.redacted);
    assert!(
        !warning_only
            .parse_warning
            .unwrap()
            .contains("warning-secret")
    );
}

#[tokio::test]
async fn escaped_source_fields_are_bounded_and_cursor_advances() {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::config::StorageConfig::for_test(dir.path().join("page.db"));
    let pool = std::sync::Arc::new(crate::db::init_pool(&storage).unwrap());
    let service = CortexService::new(pool.clone(), storage);
    let conn = pool.get().unwrap();
    for message in ["\"".repeat(140_000), "later event".into()] {
        conn.execute("INSERT INTO logs(timestamp,hostname,severity,message,raw,source_ip,ai_tool,ai_project,ai_session_id,parse_error) VALUES('2026-08-28T00:00:00Z','h','info',?1,?1,'fixture','claude','p','s',?2)", rusqlite::params![message,"warning ".repeat(50_000)]).unwrap();
    }
    drop(conn);
    let req = RenderedSessionPageRequest {
        project: "p".into(),
        tool: "claude".into(),
        session_id: "s".into(),
        host: "h".into(),
        cursor: None,
        limit: Some(1),
    };
    let first = service.rendered_session_page(req.clone()).await.unwrap();
    assert_eq!(first.events.len(), 1);
    assert!(first.high_watermark > 0);
    assert!(first.has_more);
    assert!(serde_json::to_vec(&first).unwrap().len() <= RENDERED_SESSION_PAGE_MAX_BYTES);
    let next = service
        .rendered_session_page(RenderedSessionPageRequest {
            cursor: Some(first.next_cursor),
            ..req
        })
        .await
        .unwrap();
    assert_eq!(next.events[0].text, "later event");
    assert!(next.high_watermark > first.high_watermark);
}

#[test]
fn source_page_stops_at_aggregate_byte_budget() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::init_pool(&crate::config::StorageConfig::for_test(
        dir.path().join("bounded.db"),
    ))
    .unwrap();
    let conn = pool.get().unwrap();
    for _ in 0..8 {
        conn.execute("INSERT INTO logs(timestamp,hostname,severity,message,raw,source_ip,ai_tool,ai_project,ai_session_id) VALUES('2026-08-28T00:00:00Z','h','info',?1,'','fixture','claude','p','s')", ["🦀".repeat(70_000)]).unwrap();
    }
    drop(conn);
    let (rows, more) = crate::db::rendered_session_page(
        &pool,
        &crate::db::RenderedSessionPageParams {
            ai_project: "p".into(),
            ai_tool: "claude".into(),
            ai_session_id: "s".into(),
            host: "h".into(),
            after_id: 0,
            limit: 201,
        },
    )
    .unwrap();
    assert!(more);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().map(|r| r.message.len()).sum::<usize>() <= 1024 * 1024);
    assert!(rows.iter().all(|r| {
        r.parse_error
            .as_deref()
            .unwrap()
            .contains("source fields truncated")
    }));
}
