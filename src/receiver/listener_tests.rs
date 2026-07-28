use super::*;

#[test]
fn update_backpressure_only_reports_state_transitions() {
    let mut backpressure = false;

    assert_eq!(
        update_backpressure(&mut backpressure, true),
        Some(BackpressureTransition::Applied)
    );
    assert!(backpressure);
    assert_eq!(update_backpressure(&mut backpressure, true), None);
    assert_eq!(
        update_backpressure(&mut backpressure, false),
        Some(BackpressureTransition::Cleared)
    );
    assert!(!backpressure);
    assert_eq!(update_backpressure(&mut backpressure, false), None);
}

#[tokio::test]
async fn tcp_connection_allows_multiple_lines_beyond_connection_total_size() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::db::LogBatchEntry>(16);
    let ingest = crate::ingest::IngestTx::from_sender_for_test(tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let accept_task = tokio::spawn(async move {
        let (server_stream, peer) = listener.accept().await.unwrap();
        handle_tcp_connection(server_stream, peer, ingest, 64, 5, &[]).await;
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::AsyncWriteExt;
    client
        .write_all(
            b"<34>Oct 11 22:14:15 host app: first message\n<34>Oct 11 22:14:16 host app: second message\n",
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();

    let first = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();

    assert!(first.message.contains("first message"));
    assert!(second.message.contains("second message"));

    accept_task.await.unwrap();
}

#[tokio::test]
async fn tcp_connection_closes_oversized_unterminated_line_after_bounded_drain() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::db::LogBatchEntry>(16);
    let ingest = crate::ingest::IngestTx::from_sender_for_test(tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let accept_task = tokio::spawn(async move {
        let (server_stream, peer) = listener.accept().await.unwrap();
        handle_tcp_connection(server_stream, peer, ingest, 32, 5, &[]).await;
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    client
        .write_all(&vec![b'x'; 32 * MAX_OVERSIZE_DRAIN_MULTIPLIER + 1])
        .await
        .unwrap();

    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(std::time::Duration::from_secs(1), client.read(&mut buf))
        .await
        .expect("server should close oversized TCP connection")
        .unwrap();
    assert_eq!(read, 0);

    if let Ok(Some(entry)) =
        tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
    {
        panic!(
            "oversized unterminated line must not enqueue an entry, got: {:?}",
            entry
        );
    }

    accept_task.await.unwrap();
}

#[tokio::test]
async fn tcp_connection_drops_oversized_delimited_line_and_keeps_later_frames() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::db::LogBatchEntry>(16);
    let ingest = crate::ingest::IngestTx::from_sender_for_test(tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let accept_task = tokio::spawn(async move {
        let (server_stream, peer) = listener.accept().await.unwrap();
        handle_tcp_connection(server_stream, peer, ingest, 32, 5, &[]).await;
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::AsyncWriteExt;
    client.write_all(&[b'x'; 64]).await.unwrap();
    client.write_all(b"\nvalid\n").await.unwrap();
    client.shutdown().await.unwrap();

    let entry = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(entry.raw.contains("valid"));

    accept_task.await.unwrap();
}

#[tokio::test]
async fn bounded_reader_drains_fragmented_oversize_line_and_resumes() {
    let input = format!("{}\nvalid\n", "x".repeat(64));
    let mut reader = BufReader::with_capacity(16, input.as_bytes());

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Oversize {
            line_bytes,
            terminated,
        } => {
            assert_eq!(line_bytes, 65);
            assert!(terminated);
        }
        other => panic!("expected fragmented oversized frame, got: {other:?}"),
    }

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Line(line) => assert_eq!(line, "valid"),
        other => panic!("expected valid frame after oversized frame, got: {other:?}"),
    }
}

#[test]
fn cidr_open_policy_when_empty() {
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    assert!(is_source_allowed(ip, &[]));
}

#[test]
fn cidr_v4_host_route_prefix32() {
    let target: std::net::IpAddr = "192.168.1.5".parse().unwrap();
    let other: std::net::IpAddr = "192.168.1.6".parse().unwrap();
    let cidrs = vec!["192.168.1.5/32".to_string()];
    assert!(is_source_allowed(target, &cidrs));
    assert!(!is_source_allowed(other, &cidrs));
}

#[test]
fn cidr_v4_class_c() {
    let inside: std::net::IpAddr = "10.0.0.100".parse().unwrap();
    let outside: std::net::IpAddr = "10.1.0.1".parse().unwrap();
    let cidrs = vec!["10.0.0.0/24".to_string()];
    assert!(is_source_allowed(inside, &cidrs));
    assert!(!is_source_allowed(outside, &cidrs));
}

#[test]
fn cidr_v4_prefix0_matches_all() {
    let any: std::net::IpAddr = "203.0.113.1".parse().unwrap();
    let cidrs = vec!["0.0.0.0/0".to_string()];
    assert!(is_source_allowed(any, &cidrs));
}

#[test]
fn cidr_v6_prefix128_host_route() {
    let target: std::net::IpAddr = "::1".parse().unwrap();
    let other: std::net::IpAddr = "::2".parse().unwrap();
    let cidrs = vec!["::1/128".to_string()];
    assert!(is_source_allowed(target, &cidrs));
    assert!(!is_source_allowed(other, &cidrs));
}

#[test]
fn cidr_v4_v6_mismatch_does_not_match() {
    let v4: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let cidrs = vec!["::10.0.0.0/120".to_string()]; // v6 CIDR for v4 addr
    // v4 vs v6 → no match (mismatch branch returns false)
    assert!(!is_source_allowed(v4, &cidrs));
}

#[test]
fn cidr_multiple_cidrs_any_match_allows() {
    let ip: std::net::IpAddr = "172.16.0.5".parse().unwrap();
    let cidrs = vec!["10.0.0.0/8".to_string(), "172.16.0.0/16".to_string()];
    assert!(is_source_allowed(ip, &cidrs));
}

#[test]
fn cidr_malformed_entry_is_skipped() {
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    // Only a malformed entry — should not match (no valid CIDR matches)
    let cidrs = vec!["not-a-cidr".to_string()];
    assert!(!is_source_allowed(ip, &cidrs));
}

#[test]
fn cidr_prefix_len_too_large_does_not_match() {
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let cidrs = vec!["10.0.0.0/33".to_string()]; // /33 is invalid for v4
    assert!(!is_source_allowed(ip, &cidrs));
}

#[tokio::test]
async fn bounded_reader_allows_crlf_frame_at_payload_limit() {
    let input = format!("{}\r\nnext\n", "x".repeat(32));
    let mut reader = BufReader::new(input.as_bytes());

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Line(line) => assert_eq!(line, "x".repeat(32)),
        other => panic!("expected bounded CRLF line, got unexpected frame: {other:?}"),
    }

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Line(line) => assert_eq!(line, "next"),
        other => panic!("expected next line after CRLF frame, got unexpected frame: {other:?}"),
    }
}
