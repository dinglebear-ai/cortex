use super::*;
use crate::config::StorageConfig;
use crate::db::init_pool;

fn test_pool() -> (DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("test.db"))).unwrap();
    (pool, dir)
}

fn params(dedup_key: &str, next_attempt_at: &str) -> OutboxInsertParams {
    OutboxInsertParams {
        dedup_key: dedup_key.to_string(),
        rule_id: "queue_test".to_string(),
        severity: "warning".to_string(),
        hostname: "devhost".to_string(),
        title: "Queue test".to_string(),
        body: "queued via pool wrapper".to_string(),
        apprise_urls_json: "[]".to_string(),
        next_attempt_at: next_attempt_at.to_string(),
    }
}

#[test]
fn enqueue_and_claim_pending_round_trip_through_pool() {
    let (pool, _dir) = test_pool();

    enqueue(&pool, &params("queue-ready", "2000-01-01T00:00:00.000Z")).unwrap();
    let rows = claim_pending(&pool, 10).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].dedup_key, "queue-ready");
    assert_eq!(rows[0].hostname, "devhost");
}

#[test]
fn claim_pending_honors_limit_and_next_attempt_at() {
    let (pool, _dir) = test_pool();
    enqueue(&pool, &params("ready-a", "2000-01-01T00:00:00.000Z")).unwrap();
    enqueue(&pool, &params("ready-b", "2000-01-01T00:00:01.000Z")).unwrap();
    enqueue(&pool, &params("future", "2999-01-01T00:00:00.000Z")).unwrap();

    let rows = claim_pending(&pool, 1).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].dedup_key, "ready-a");
}

#[test]
fn enqueue_preserves_pending_dedup_idempotency() {
    let (pool, _dir) = test_pool();
    let insert = params("same-key", "2000-01-01T00:00:00.000Z");

    enqueue(&pool, &insert).unwrap();
    enqueue(&pool, &insert).unwrap();

    let rows = claim_pending(&pool, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].dedup_key, "same-key");
}
