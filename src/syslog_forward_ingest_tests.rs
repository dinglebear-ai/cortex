use super::*;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};

#[derive(Clone)]
struct ReceiptLossServerState {
    receiver: SyslogForwardIngestState,
    /// Models the only ambiguous network outcome that matters here: Cortex
    /// commits the transaction, but the sender never receives the receipt.
    lose_next_successful_receipt: Arc<AtomicBool>,
}

async fn receipt_loss_handler(
    State(state): State<ReceiptLossServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let response = ingest_handler(State(state.receiver), ConnectInfo(peer), headers, body).await;

    if response.status().is_success()
        && state
            .lose_next_successful_receipt
            .swap(false, Ordering::SeqCst)
    {
        // The request has already crossed the receiver's authenticated,
        // transactional boundary. Returning 503 intentionally discards its
        // receipt from the sender's point of view, forcing an exact replay.
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        response
    }
}

async fn wait_for_sender(
    sender: &crate::agent::syslog_sender::SyslogSender,
    predicate: impl Fn(&crate::agent::syslog_sender::SyslogForwardStatus) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = sender.status();
            if predicate(&status) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("syslog sender did not reach the expected recovery state");
}

#[tokio::test]
async fn sender_replays_an_authenticated_receipt_loss_once_after_outage_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let spool_path = dir.path().join("forward-spool.json");
    let token = "syslog-forward-test-token".to_owned();

    // Reserve then release this address so the original sender experiences a
    // genuine transport outage, not a mocked client error.
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let target = format!("http://{address}");

    let original_sender = crate::agent::syslog_sender::SyslogSender::new(
        target.clone(),
        Some(token.clone()),
        spool_path.clone(),
    );
    original_sender
        .send_from(
            "journald",
            "<134>1 2026-09-01T00:00:00Z agent-a cortex-agent - - - recovery-proof".to_owned(),
        )
        .await
        .unwrap();
    wait_for_sender(&original_sender, |status| {
        status.queued_records == 1 && status.last_error_code == Some("transport_unavailable")
    })
    .await;
    assert!(spool_path.exists(), "outage must be durable before restart");

    // Dropping the previous agent stops its delivery task. A new process then
    // loads exactly the same spool file after the receiver returns.
    drop(original_sender);

    let pool = Arc::new(
        crate::db::init_pool(&crate::config::StorageConfig::for_test(
            dir.path().join("syslog-forward-recovery.db"),
        ))
        .unwrap(),
    );
    let receiver = SyslogForwardIngestState::new(
        Arc::clone(&pool),
        Some(token),
        crate::mcp::AuthPolicy::TrustedGatewayUnscoped,
    );
    let loss_state = ReceiptLossServerState {
        receiver,
        lose_next_successful_receipt: Arc::new(AtomicBool::new(true)),
    };
    let app = Router::new()
        .route("/v1/syslog-forward", post(receipt_loss_handler))
        .with_state(loss_state);
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let receiver_task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .unwrap();
    });

    let restarted_sender = crate::agent::syslog_sender::SyslogSender::new(
        target,
        Some("syslog-forward-test-token".to_owned()),
        spool_path,
    );
    wait_for_sender(&restarted_sender, |status| status.queued_records == 0).await;
    drop(restarted_sender);
    let _ = shutdown_tx.send(());
    receiver_task.await.unwrap();

    let conn = pool.get().unwrap();
    let evidence_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs WHERE message = 'recovery-proof'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let receipt_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM syslog_forward_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        evidence_count, 1,
        "receipt-loss replay must not duplicate evidence"
    );
    assert_eq!(receipt_count, 1, "the exact replay must reuse one receipt");
}

#[test]
fn records_reject_unsafe_or_unbounded_inputs() {
    let valid = SyslogForwardRecord {
        source_instance: "host-a".into(),
        source_epoch: 1,
        sequence: 1,
        idempotency_key: "key".into(),
        observed_at: "2026-01-01T00:00:00Z".into(),
        line: "<134>1 ts host app - - - message".into(),
    };
    assert!(!invalid_record(&valid));
    assert!(invalid_record(&SyslogForwardRecord {
        line: String::new(),
        ..valid
    }));
}

#[test]
fn gap_rejects_reversed_window() {
    assert!(invalid_gap(&SyslogForwardGap {
        source_instance: "host-a".into(),
        source_epoch: 1,
        from_sequence: 2,
        to_sequence: 1,
        idempotency_key: "gap".into(),
        observed_at: "2026-01-01T00:00:00Z".into(),
        reason_code: "local_retention_quota".into()
    }));
}

#[test]
fn duplicate_replay_returns_receipt_without_duplicate_canonical_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(
        crate::db::init_pool(&crate::config::StorageConfig::for_test(
            dir.path().join("syslog-forward.db"),
        ))
        .unwrap(),
    );
    let request = SyslogForwardRequest {
        records: vec![SyslogForwardRecord {
            source_instance: "host-a".into(),
            source_epoch: 1,
            sequence: 1,
            idempotency_key: "host-a:1:1:record".into(),
            observed_at: "2026-01-01T00:00:00Z".into(),
            line: "<134>1 2026-01-01T00:00:00Z host-a app - - - replay-safe".into(),
        }],
        gaps: vec![],
    };
    let first = persist_request(&pool, request.clone(), "127.0.0.1").unwrap();
    let replay = persist_request(&pool, request, "127.0.0.1").unwrap();
    assert_eq!(first, replay);
    let conn = pool.get().unwrap();
    let logs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs WHERE message = 'replay-safe'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let receipts: i64 = conn
        .query_row("SELECT COUNT(*) FROM syslog_forward_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(logs, 1);
    assert_eq!(receipts, 1);
}

#[test]
fn receiver_never_persists_a_path_like_source_identity() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(
        crate::db::init_pool(&crate::config::StorageConfig::for_test(
            dir.path().join("syslog-forward-private.db"),
        ))
        .unwrap(),
    );
    let source = "/Users/jmagar/private/production/access.log";
    persist_request(
        &pool,
        SyslogForwardRequest {
            records: vec![SyslogForwardRecord {
                source_instance: source.into(),
                source_epoch: 1,
                sequence: 1,
                idempotency_key: format!("{source}:1"),
                observed_at: "2026-01-01T00:00:00Z".into(),
                line: "<134>1 2026-01-01T00:00:00Z host-a app - - - safe".into(),
            }],
            gaps: vec![],
        },
        "127.0.0.1",
    )
    .unwrap();
    let conn = pool.get().unwrap();
    let receipt_source: String = conn
        .query_row(
            "SELECT source_instance FROM syslog_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let receipt_key: String = conn
        .query_row(
            "SELECT idempotency_key FROM syslog_forward_receipts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!receipt_source.contains(source));
    assert!(!receipt_key.contains(source));
}
