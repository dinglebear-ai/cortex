use super::*;

use crate::config::StorageConfig;
use crate::db::init_pool;

fn pool() -> (tempfile::TempDir, DbPool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("metrics.db"))).unwrap();
    (dir, pool)
}

fn point(key_byte: u8) -> OtelMetricPointInput {
    OtelMetricPointInput {
        point_key: format!("{key_byte:02x}").repeat(32),
        metric_name: "agent.tokens".into(),
        description: "token count".into(),
        unit: "{token}".into(),
        instrument_kind: "sum".into(),
        aggregation_temporality: Some(2),
        monotonic: Some(true),
        start_time_unix_nano: Some(100),
        time_unix_nano: 200,
        hostname: "fixture-host".into(),
        service_name: Some("codex".into()),
        service_version: None,
        scope_name: Some("fixture".into()),
        scope_version: Some("1".into()),
        ai_tool: Some("codex".into()),
        ai_project: Some("/workspace/cortex".into()),
        ai_session_id: Some("session-1".into()),
        run_id: None,
        resource_json: r#"{"resource":{},"scope":{}}"#.into(),
        attributes_json: r#"{"model":"fixture"}"#.into(),
        value_json: r#"{"type":"int","value":42}"#.into(),
        exemplars_json: "[]".into(),
        received_at: "2026-08-21T12:00:00.000Z".into(),
        content_scrubbed: true,
    }
}

#[test]
fn repeated_metric_batch_is_idempotent() {
    let (_dir, pool) = pool();
    let input = point(0x11);
    assert_eq!(
        insert_otel_metric_points_batch(&pool, std::slice::from_ref(&input)).unwrap(),
        OtelMetricBatchResult {
            accepted: 1,
            duplicates: 0,
            rejected: 0,
        }
    );
    assert_eq!(
        insert_otel_metric_points_batch(&pool, &[input]).unwrap(),
        OtelMetricBatchResult {
            accepted: 0,
            duplicates: 1,
            rejected: 0,
        }
    );
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM otel_metric_points", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn concurrent_metric_batches_store_one_point() {
    let (_dir, pool) = pool();
    let results = std::thread::scope(|scope| {
        let first_pool = pool.clone();
        let second_pool = pool.clone();
        let first =
            scope.spawn(move || insert_otel_metric_points_batch(&first_pool, &[point(0x44)]));
        let second =
            scope.spawn(move || insert_otel_metric_points_batch(&second_pool, &[point(0x44)]));
        [
            first.join().unwrap().unwrap(),
            second.join().unwrap().unwrap(),
        ]
    });
    assert_eq!(
        results.iter().map(|result| result.accepted).sum::<usize>(),
        1
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.duplicates)
            .sum::<usize>(),
        1
    );
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM otel_metric_points", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn malformed_metric_does_not_poison_valid_neighbor() {
    let (_dir, pool) = pool();
    let valid = point(0x22);
    let mut invalid = point(0x33);
    invalid.value_json = "not-json".into();
    let result = insert_otel_metric_points_batch(&pool, &[invalid, valid]).unwrap();
    assert_eq!(
        result,
        OtelMetricBatchResult {
            accepted: 1,
            duplicates: 0,
            rejected: 1,
        }
    );
}

#[test]
fn nonexistent_run_and_oversized_metadata_are_rejected() {
    let (_dir, pool) = pool();
    let mut missing_run = point(0x44);
    missing_run.run_id = Some(999_999);
    let mut oversized = point(0x55);
    oversized.attributes_json = format!(r#"{{"value":"{}"}}"#, "x".repeat(256 * 1024));
    let result = insert_otel_metric_points_batch(&pool, &[missing_run, oversized]).unwrap();
    assert_eq!(result.rejected, 2);
    assert_eq!(result.accepted, 0);
}

#[test]
fn invalid_instrument_metadata_combinations_are_rejected() {
    let (_dir, pool) = pool();
    let mut invalid = Vec::new();

    let mut gauge_with_sum_metadata = point(0x61);
    gauge_with_sum_metadata.instrument_kind = "gauge".into();
    invalid.push(gauge_with_sum_metadata);

    let mut sum_without_temporality = point(0x62);
    sum_without_temporality.aggregation_temporality = None;
    invalid.push(sum_without_temporality);

    let mut sum_without_monotonic = point(0x63);
    sum_without_monotonic.monotonic = None;
    invalid.push(sum_without_monotonic);

    let mut histogram_with_monotonic = point(0x64);
    histogram_with_monotonic.instrument_kind = "histogram".into();
    invalid.push(histogram_with_monotonic);

    let mut summary_with_temporality = point(0x65);
    summary_with_temporality.instrument_kind = "summary".into();
    summary_with_temporality.monotonic = None;
    invalid.push(summary_with_temporality);

    let result = insert_otel_metric_points_batch(&pool, &invalid).unwrap();
    assert_eq!(result.rejected, invalid.len());
    assert_eq!(result.accepted, 0);
}

#[test]
fn valid_metadata_for_each_instrument_kind_is_accepted() {
    let (_dir, pool) = pool();
    let mut entries = Vec::new();

    let mut gauge = point(0x71);
    gauge.instrument_kind = "gauge".into();
    gauge.aggregation_temporality = None;
    gauge.monotonic = None;
    entries.push(gauge);

    entries.push(point(0x72));

    let mut histogram = point(0x73);
    histogram.instrument_kind = "histogram".into();
    histogram.monotonic = None;
    entries.push(histogram);

    let mut exponential = point(0x74);
    exponential.instrument_kind = "exponential_histogram".into();
    exponential.monotonic = None;
    entries.push(exponential);

    let mut summary = point(0x75);
    summary.instrument_kind = "summary".into();
    summary.aggregation_temporality = None;
    summary.monotonic = None;
    entries.push(summary);

    let result = insert_otel_metric_points_batch(&pool, &entries).unwrap();
    assert_eq!(result.accepted, entries.len());
    assert_eq!(result.rejected, 0);
}
