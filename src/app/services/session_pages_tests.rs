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
