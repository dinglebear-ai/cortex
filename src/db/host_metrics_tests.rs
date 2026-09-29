use super::*;
use crate::config::StorageConfig;

#[test]
fn filters_and_paginates_unlinked_host_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let pool =
        crate::db::init_pool(&StorageConfig::for_test(dir.path().join("host-metrics.db"))).unwrap();
    let conn = pool.get().unwrap();
    for (id, host, service, metric, timestamp) in [
        (
            1,
            "host-a",
            "cortex-hostmetrics",
            "system.cpu.utilization",
            100,
        ),
        (
            2,
            "host-a",
            "cortex-hostmetrics",
            "system.cpu.utilization",
            200,
        ),
        (
            3,
            "host-a",
            "cortex-hostmetrics",
            "system.cpu.utilization",
            300,
        ),
        (
            4,
            "other",
            "cortex-hostmetrics",
            "system.cpu.utilization",
            400,
        ),
        (5, "host-a", "other-service", "system.cpu.utilization", 500),
        (
            6,
            "host-a",
            "cortex-hostmetrics",
            "system.memory.utilization",
            600,
        ),
    ] {
        conn.execute(
            "INSERT INTO otel_metric_points
             (point_key,metric_name,instrument_kind,time_unix_nano,hostname,service_name,
              value_json,attributes_json,received_at)
             VALUES (?1,?2,'gauge',?3,?4,?5,'{\"type\":\"double\",\"value\":0.5}',
                     '{\"state\":\"used\"}','2026-09-29T14:00:00Z')",
            rusqlite::params![format!("point-{id}"), metric, timestamp, host, service],
        )
        .unwrap();
    }
    drop(conn);

    let mut params = HostMetricParams {
        hostname: "host-a".into(),
        service_name: Some("cortex-hostmetrics".into()),
        metric_name: "system.cpu.utilization".into(),
        since_unix_nano: 100,
        before: None,
        limit: 2,
    };
    let first = list_host_metrics(&pool, &params).unwrap();
    assert_eq!(
        first
            .points
            .iter()
            .map(|p| p.time_unix_nano)
            .collect::<Vec<_>>(),
        vec![300, 200]
    );
    assert_eq!(first.points[0].value["value"], 0.5);
    assert_eq!(first.points[0].attributes["state"], "used");
    assert!(first.truncated);
    params.before = first.next_cursor;
    let second = list_host_metrics(&pool, &params).unwrap();
    assert_eq!(second.points.len(), 1);
    assert_eq!(second.points[0].time_unix_nano, 100);
    assert!(!second.truncated);
    assert!(second.next_cursor.is_none());

    let metric_hosts = list_metric_hosts(&pool, 500).unwrap();
    assert_eq!(metric_hosts.len(), 1);
    assert_eq!(metric_hosts[0].hostname, "host-a");
    assert_eq!(metric_hosts[0].latest_time_unix_nano, 600);
}
