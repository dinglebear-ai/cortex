use super::*;

#[tokio::test]
async fn accepted_otlp_identifier_boundaries_are_queryable() {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::config::StorageConfig::for_test(dir.path().join("metrics.db"));
    let pool = std::sync::Arc::new(db::init_pool(&storage).unwrap());
    let hostname = "é".repeat(255);
    let metric_name = "m".repeat(512);
    let service_name = "s".repeat(512);
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO otel_metric_points
         (point_key,metric_name,instrument_kind,time_unix_nano,hostname,service_name,
          value_json,received_at)
         VALUES ('boundary',?4,'gauge',?1,?2,?3,'{}','2026-09-30T00:00:00Z')",
            rusqlite::params![
                Utc::now().timestamp_nanos_opt().unwrap(),
                hostname,
                service_name,
                metric_name
            ],
        )
        .unwrap();
    let service = CortexService::new(pool, storage);
    let req = ListHostMetricsRequest {
        hostname,
        metric_name,
        service_name: Some(service_name),
        ..Default::default()
    };
    let response = service.list_host_metrics(req.clone()).await.unwrap();
    assert_eq!(response.points.len(), 1);

    for oversized in [
        ListHostMetricsRequest {
            hostname: "h".repeat(256),
            ..req.clone()
        },
        ListHostMetricsRequest {
            metric_name: "m".repeat(513),
            ..req.clone()
        },
        ListHostMetricsRequest {
            service_name: Some("s".repeat(513)),
            ..req
        },
    ] {
        assert!(matches!(
            service.list_host_metrics(oversized).await,
            Err(ServiceError::InvalidInput(_))
        ));
    }
}
