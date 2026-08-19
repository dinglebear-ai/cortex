use super::*;

use crate::config::StorageConfig;
use crate::db::init_pool;

fn pool(name: &str) -> (tempfile::TempDir, DbPool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join(name))).unwrap();
    (dir, pool)
}

fn span(trace: u8, span: u8) -> OtelSpanInput {
    OtelSpanInput {
        trace_id: format!("{trace:02x}").repeat(16),
        span_id: format!("{span:02x}").repeat(8),
        parent_span_id: Some("33".repeat(8)),
        trace_state: Some("vendor=value".to_string()),
        flags: 0x101,
        span_name: "tool.call".to_string(),
        span_kind: 3,
        start_time_unix_nano: 1_700_000_000_000_000_000,
        end_time_unix_nano: 1_700_000_000_000_025_000,
        duration_nano: 25_000,
        status_code: 2,
        status_message: Some("boom".to_string()),
        hostname: "devhost".to_string(),
        service_name: Some("claude-code".to_string()),
        service_version: Some("1.2.3".to_string()),
        scope_name: Some("cortex.trace.tests".to_string()),
        scope_version: Some("0.1.0".to_string()),
        ai_tool: Some("claude".to_string()),
        ai_project: Some("/workspace/cortex".to_string()),
        ai_session_id: Some("session-123".to_string()),
        run_id: None,
        resource_json: r#"{"resource":{"attributes":{}},"scope":{"attributes":{}}}"#.to_string(),
        attributes_json: r#"{"custom":"value"}"#.to_string(),
        events_json: r#"[{"time_unix_nano":1,"name":"event"}]"#.to_string(),
        links_json:
            r#"[{"trace_id":"44444444444444444444444444444444","span_id":"5555555555555555"}]"#
                .to_string(),
        received_at: "2026-08-18T20:15:00.000Z".to_string(),
        content_scrubbed: true,
    }
}

fn row_count(pool: &DbPool) -> i64 {
    pool.get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn empty_batch_is_a_noop() {
    let (_dir, pool) = pool("empty.db");
    let result = insert_otel_spans_batch(&pool, &[]).unwrap();
    assert_eq!(result, OtelTraceBatchResult::default());
    assert_eq!(result.total(), 0);
    assert_eq!(row_count(&pool), 0);
}

#[test]
fn duplicate_export_is_idempotent_and_reported_as_duplicate_not_rejected() {
    let (_dir, pool) = pool("duplicate.db");
    let input = span(0x11, 0x22);

    let first = insert_otel_spans_batch(&pool, std::slice::from_ref(&input)).unwrap();
    assert_eq!(
        first,
        OtelTraceBatchResult {
            accepted: 1,
            duplicates: 0,
            rejected: 0,
        }
    );
    let second = insert_otel_spans_batch(&pool, std::slice::from_ref(&input)).unwrap();
    assert_eq!(
        second,
        OtelTraceBatchResult {
            accepted: 0,
            duplicates: 1,
            rejected: 0,
        }
    );
    assert_eq!(row_count(&pool), 1);

    let conn = pool.get().unwrap();
    let persisted: (String, String, String, i64, String, String, String, bool) = conn
        .query_row(
            "SELECT trace_id, span_id, span_name, duration_nano, attributes_json,
                    events_json, links_json, content_scrubbed
               FROM otel_spans",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(persisted.0, input.trace_id);
    assert_eq!(persisted.1, input.span_id);
    assert_eq!(persisted.2, input.span_name);
    assert_eq!(persisted.3, input.duration_nano);
    assert_eq!(persisted.4, input.attributes_json);
    assert_eq!(persisted.5, input.events_json);
    assert_eq!(persisted.6, input.links_json);
    assert!(persisted.7);
}

#[test]
fn duplicate_inside_one_batch_counts_one_accept_and_one_duplicate() {
    let (_dir, pool) = pool("same-batch.db");
    let input = span(0x11, 0x22);
    let result = insert_otel_spans_batch(&pool, &[input.clone(), input]).unwrap();
    assert_eq!(
        result,
        OtelTraceBatchResult {
            accepted: 1,
            duplicates: 1,
            rejected: 0,
        }
    );
    assert_eq!(result.total(), 2);
    assert_eq!(row_count(&pool), 1);
}

#[test]
fn malformed_rows_are_rejected_without_poisoning_valid_neighbors() {
    let (_dir, pool) = pool("mixed.db");
    let valid = span(0x11, 0x22);
    let mut invalid = Vec::new();

    let mut bad = span(0x21, 0x31);
    bad.trace_id = "0".repeat(32);
    invalid.push(bad);

    let mut bad = span(0x22, 0x32);
    bad.span_id = "zz".repeat(8);
    invalid.push(bad);

    let mut bad = span(0x23, 0x33);
    bad.parent_span_id = Some("0".repeat(16));
    invalid.push(bad);

    let mut bad = span(0x24, 0x34);
    bad.duration_nano += 1;
    invalid.push(bad);

    let mut bad = span(0x25, 0x35);
    bad.attributes_json = "not-json".to_string();
    invalid.push(bad);

    let mut bad = span(0x26, 0x36);
    bad.events_json = "{}".to_string();
    invalid.push(bad);

    let mut bad = span(0x27, 0x37);
    bad.received_at = "not-a-time".to_string();
    invalid.push(bad);

    let mut bad = span(0x28, 0x38);
    bad.run_id = Some(9_999_999);
    invalid.push(bad);

    let mut entries = vec![valid.clone()];
    entries.extend(invalid);
    let result = insert_otel_spans_batch(&pool, &entries).unwrap();
    assert_eq!(
        result,
        OtelTraceBatchResult {
            accepted: 1,
            duplicates: 0,
            rejected: 8,
        }
    );
    assert_eq!(result.total(), entries.len());
    assert_eq!(row_count(&pool), 1);

    let conn = pool.get().unwrap();
    let only: (String, String) = conn
        .query_row("SELECT trace_id, span_id FROM otel_spans", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(only, (valid.trace_id, valid.span_id));
}

#[test]
fn metadata_size_and_flattened_field_bounds_reject_direct_db_bypass() {
    let (_dir, pool) = pool("bounds.db");
    let mut oversized_json = span(0x11, 0x22);
    oversized_json.attributes_json = serde_json::json!({
        "payload": "x".repeat(MAX_METADATA_JSON_BYTES)
    })
    .to_string();

    let mut oversized_name = span(0x12, 0x23);
    oversized_name.span_name = "n".repeat(MAX_SPAN_NAME_CHARS + 1);

    let mut oversized_tool = span(0x13, 0x24);
    oversized_tool.ai_tool = Some("t".repeat(MAX_TOOL_BYTES + 1));

    let result =
        insert_otel_spans_batch(&pool, &[oversized_json, oversized_name, oversized_tool]).unwrap();
    assert_eq!(result.accepted, 0);
    assert_eq!(result.duplicates, 0);
    assert_eq!(result.rejected, 3);
    assert_eq!(row_count(&pool), 0);
}
