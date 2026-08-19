use super::is_unaddressed_warning_noise;
use crate::config::StorageConfig;
use std::sync::Arc;

#[test]
fn unaddressed_warning_noise_filters_health_checks_only() {
    assert!(is_unaddressed_warning_noise(
        "warning",
        "GET request for '/' received from 127.0.0.1 using 'curl/8.0'",
        "GET response status for '/' in 0.000 seconds plain 19 bytes: 302 Found",
    ));
    assert!(is_unaddressed_warning_noise(
        "warning",
        "tool list ok",
        "labby tool list ok in 44ms",
    ));
    assert!(is_unaddressed_warning_noise(
        "warning",
        "GET /.well-known/oauth-authorization-server => generated 404",
        "GET /.well-known/oauth-authorization-server",
    ));
    assert!(is_unaddressed_warning_noise(
        "warning",
        "GET /health => generated 200",
        "GET /health => generated HTTP 200",
    ));
    assert!(is_unaddressed_warning_noise(
        "warning",
        "skipping mandatory platform policies because no policy file was found",
        "skipping mandatory platform policies because no policy file was found",
    ));
    assert!(!is_unaddressed_warning_noise(
        "warning",
        "auth failure for admin",
        "failed auth for admin after tool list ok",
    ));
    assert!(!is_unaddressed_warning_noise(
        "err",
        "GET /health => generated",
        "GET /health => generated HTTP 200",
    ));
    assert!(!is_unaddressed_warning_noise(
        "warning",
        "imfile: cannot open file",
        "Permission denied reading /home/jmagar/.claude/projects/session.jsonl",
    ));
}

#[tokio::test]
async fn unaddressed_errors_pages_past_filtered_warning_noise() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("unaddressed-errors.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    {
        let conn = pool.get().unwrap();
        for i in 0..60 {
            let ts = format!("2026-01-01T00:{i:02}:00.000Z");
            crate::db::error_signatures::upsert_signature(
                &conn,
                crate::db::error_signatures::UpsertSignatureParams {
                    hash: &format!("{i:064x}"),
                    normalizer_version: crate::app::error_detection::NORMALIZER_VERSION,
                    template: "tool list ok",
                    sample_message: "labby tool list ok in 44ms",
                    sample_hostname: "devhost",
                    sample_app_name: Some("labby"),
                    severity: "warning",
                    first_seen_at: &ts,
                    last_seen_at: &ts,
                    delta: 1,
                },
            )
            .unwrap();
        }
        crate::db::error_signatures::upsert_signature(
            &conn,
            crate::db::error_signatures::UpsertSignatureParams {
                hash: &format!("{:064x}", 10_000),
                normalizer_version: crate::app::error_detection::NORMALIZER_VERSION,
                template: "disk full on /var",
                sample_message: "disk full on /var",
                sample_hostname: "devhost",
                sample_app_name: Some("kernel"),
                severity: "err",
                first_seen_at: "2026-01-01T00:00:00.000Z",
                last_seen_at: "2025-12-31T23:59:00.000Z",
                delta: 1,
            },
        )
        .unwrap();
    }

    let svc = crate::app::CortexService::new(Arc::clone(&pool), storage);
    let resp = svc
        .unaddressed_errors(crate::app::models::UnaddressedErrorsRequest {
            limit: Some(1),
            include_acknowledged: None,
        })
        .await
        .unwrap();

    assert_eq!(resp.signatures.len(), 1);
    assert_eq!(resp.signatures[0].template, "disk full on /var");
    assert_eq!(resp.filtered_count, 60);
    assert!(resp.candidate_rows > 50);
    assert!(!resp.candidate_window_truncated);
}

#[tokio::test]
async fn unaddressed_errors_hides_non_current_normalizer_versions() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("unaddressed-version.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    let current_version = crate::app::error_detection::NORMALIZER_VERSION;
    {
        let conn = pool.get().unwrap();
        crate::db::error_signatures::upsert_signature(
            &conn,
            crate::db::error_signatures::UpsertSignatureParams {
                hash: "active-version",
                normalizer_version: current_version,
                template: "active disk error",
                sample_message: "active disk error",
                sample_hostname: "devhost",
                sample_app_name: Some("kernel"),
                severity: "err",
                first_seen_at: "2026-01-01T00:00:00.000Z",
                last_seen_at: "2026-01-01T00:00:00.000Z",
                delta: 1,
            },
        )
        .unwrap();
        crate::db::error_signatures::upsert_signature(
            &conn,
            crate::db::error_signatures::UpsertSignatureParams {
                hash: "foreign-version",
                normalizer_version: current_version + 1,
                template: "newer foreign-version error",
                sample_message: "newer foreign-version error",
                sample_hostname: "devhost",
                sample_app_name: Some("kernel"),
                severity: "err",
                first_seen_at: "2026-01-02T00:00:00.000Z",
                last_seen_at: "2026-01-02T00:00:00.000Z",
                delta: 1,
            },
        )
        .unwrap();
    }

    let svc = crate::app::CortexService::new(Arc::clone(&pool), storage);
    let resp = svc
        .unaddressed_errors(crate::app::models::UnaddressedErrorsRequest {
            limit: Some(10),
            include_acknowledged: None,
        })
        .await
        .unwrap();

    assert_eq!(resp.signatures.len(), 1);
    assert_eq!(resp.signatures[0].signature_hash, "active-version");
    let conn = pool.get().unwrap();
    let foreign_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM error_signatures WHERE signature_hash = 'foreign-version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(foreign_rows, 1, "historical versions stay durable");
}

#[tokio::test]
async fn unaddressed_errors_reports_truncation_when_scan_cap_hit() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("unaddressed-truncate.db"));
    let pool = Arc::new(crate::db::init_pool(&storage).unwrap());
    {
        let conn = pool.get().unwrap();
        // Seed more than UNADDRESSED_SCAN_CAP (5000) warning-noise rows and zero
        // real errors so the scan hits the cap before filling the limit. One
        // transaction keeps the bulk insert fast.
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..5_050u32 {
            let ts = format!("2026-01-01T00:00:{:02}.{:03}Z", i % 60, i / 60);
            crate::db::error_signatures::upsert_signature(
                &conn,
                crate::db::error_signatures::UpsertSignatureParams {
                    hash: &format!("{i:064x}"),
                    normalizer_version: crate::app::error_detection::NORMALIZER_VERSION,
                    template: "tool list ok",
                    sample_message: "labby tool list ok in 44ms",
                    sample_hostname: "devhost",
                    sample_app_name: Some("labby"),
                    severity: "warning",
                    first_seen_at: &ts,
                    last_seen_at: &ts,
                    delta: 1,
                },
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
    }

    let svc = crate::app::CortexService::new(Arc::clone(&pool), storage);
    let resp = svc
        .unaddressed_errors(crate::app::models::UnaddressedErrorsRequest {
            limit: Some(1),
            include_acknowledged: None,
        })
        .await
        .unwrap();

    assert!(
        resp.signatures.is_empty(),
        "every scanned candidate is noise, so the page is empty"
    );
    assert!(
        resp.candidate_window_truncated,
        "scan hit the cap before filling the limit"
    );
    assert!(resp.candidate_rows >= resp.candidate_cap);
    assert_eq!(resp.candidate_cap, 5_000);
}
