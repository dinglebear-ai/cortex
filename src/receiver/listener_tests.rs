use super::*;

fn cidrs(values: &[&str]) -> Vec<IpNet> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

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
async fn tcp_connection_preserves_all_lines_when_ingest_queue_is_saturated() {
    // A one-slot downstream queue forces the listener to exercise backpressure
    // almost immediately. TCP must await capacity rather than convert reliable
    // delivery into best-effort try_send drops.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::db::LogBatchEntry>(1);
    let ingest = crate::ingest::IngestTx::from_sender_for_test(tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let accept_task = tokio::spawn(async move {
        let (server_stream, peer) = listener.accept().await.unwrap();
        handle_tcp_connection(server_stream, peer, ingest, 256, 5, &[]).await;
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::AsyncWriteExt;
    let payload = (0..100)
        .map(|index| format!("<34>Oct 11 22:14:15 host app: pressure-{index}\n"))
        .collect::<String>();
    client.write_all(payload.as_bytes()).await.unwrap();
    client.shutdown().await.unwrap();

    let mut received = Vec::with_capacity(100);
    for _ in 0..100 {
        let entry = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .expect("TCP listener should resume once downstream capacity is available")
            .expect("ingest bridge must stay open");
        received.push(entry.message);
    }

    for (index, message) in received.iter().enumerate() {
        assert!(
            message.contains(&format!("pressure-{index}")),
            "TCP frame {index} was lost or reordered under queue pressure: {message:?}"
        );
    }
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

/// An at-limit payload terminated by CRLF must be accepted regardless of where
/// the read boundary falls. Sizing the buffer so the fill ends exactly on the
/// `\r` puts the `\n` at position 0 of the next chunk, which is the split that
/// previously classified a legal frame as oversized based purely on TCP
/// segmentation.
#[tokio::test]
async fn bounded_reader_accepts_at_limit_crlf_frame_split_on_the_carriage_return() {
    let payload = "x".repeat(32);
    let input = format!("{payload}\r\nnext\n");
    let mut reader = BufReader::with_capacity(33, input.as_bytes());

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Line(line) => assert_eq!(line, payload),
        other => panic!("expected at-limit CRLF frame to be accepted, got: {other:?}"),
    }

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Line(line) => assert_eq!(line, "next"),
        other => panic!("expected the following frame, got: {other:?}"),
    }
}

/// One byte of payload past the limit is still oversized when the CRLF splits
/// the same way — the extra accumulate byte is reserved for the terminator, not
/// for payload.
#[tokio::test]
async fn bounded_reader_rejects_over_limit_crlf_frame_split_on_the_carriage_return() {
    let input = format!("{}\r\nnext\n", "x".repeat(33));
    let mut reader = BufReader::with_capacity(34, input.as_bytes());

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Oversize { terminated, .. } => assert!(terminated),
        other => panic!("expected over-limit CRLF frame to be dropped, got: {other:?}"),
    }

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Line(line) => assert_eq!(line, "next"),
        other => panic!("expected the stream to resume, got: {other:?}"),
    }
}

/// EOF part-way through a drain must report the frame as unterminated, not as
/// a clean end of stream — otherwise a peer that truncates mid-flood is logged
/// as `eof` and the abuse signal is lost.
#[tokio::test]
async fn bounded_reader_reports_unterminated_frame_when_eof_interrupts_a_drain() {
    let input = [b'x'; 64];
    let mut reader = BufReader::with_capacity(16, &input[..]);

    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Oversize {
            line_bytes,
            terminated,
        } => {
            assert_eq!(line_bytes, 64);
            assert!(!terminated, "EOF mid-drain must not report a terminator");
        }
        other => panic!("expected an unterminated oversized frame, got: {other:?}"),
    }
}

/// The drain bound has two halves: an oversize run that terminates within the
/// budget resumes the stream, and one that never terminates is cut off. Assert
/// the property rather than an exact byte count — the budget is only checked on
/// delimiter-free chunks, so a frame can overrun it by up to one chunk.
#[tokio::test]
async fn bounded_reader_resumes_within_the_drain_budget_and_cuts_off_beyond_it() {
    let within = format!(
        "{}\nvalid\n",
        "x".repeat(32 * MAX_OVERSIZE_DRAIN_MULTIPLIER)
    );
    let mut reader = BufReader::with_capacity(16, within.as_bytes());
    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Oversize { terminated, .. } => {
            assert!(
                terminated,
                "a frame ending within the budget must terminate"
            );
        }
        other => panic!("expected a terminated oversized frame, got: {other:?}"),
    }
    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Line(line) => assert_eq!(line, "valid"),
        other => panic!("expected the stream to resume after the drain, got: {other:?}"),
    }

    let beyond = vec![b'x'; 32 * (MAX_OVERSIZE_DRAIN_MULTIPLIER + 4)];
    let mut reader = BufReader::with_capacity(16, &beyond[..]);
    match read_bounded_line(&mut reader, 32).await.unwrap() {
        TcpFrame::Oversize { terminated, .. } => {
            assert!(!terminated, "an unterminated flood must be cut off");
        }
        other => panic!("expected the flood to be cut off, got: {other:?}"),
    }
}

#[test]
fn oversize_logging_backs_off_exponentially() {
    let logged: Vec<u64> = (1..=1000).filter(|n| should_log_oversize(*n)).collect();
    assert_eq!(logged, vec![1, 10, 100, 1000]);
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
    let cidrs = cidrs(&["192.168.1.5/32"]);
    assert!(is_source_allowed(target, &cidrs));
    assert!(!is_source_allowed(other, &cidrs));
}

#[test]
fn cidr_v4_class_c() {
    let inside: std::net::IpAddr = "10.0.0.100".parse().unwrap();
    let outside: std::net::IpAddr = "192.0.2.1".parse().unwrap();
    let cidrs = cidrs(&["10.0.0.0/24"]);
    assert!(is_source_allowed(inside, &cidrs));
    assert!(!is_source_allowed(outside, &cidrs));
}

#[test]
fn cidr_v4_prefix0_matches_all() {
    let any: std::net::IpAddr = "203.0.113.1".parse().unwrap();
    let cidrs = cidrs(&["0.0.0.0/0"]);
    assert!(is_source_allowed(any, &cidrs));
}

#[test]
fn cidr_v6_prefix128_host_route() {
    let target: std::net::IpAddr = "::1".parse().unwrap();
    let other: std::net::IpAddr = "::2".parse().unwrap();
    let cidrs = cidrs(&["::1/128"]);
    assert!(is_source_allowed(target, &cidrs));
    assert!(!is_source_allowed(other, &cidrs));
}

#[test]
fn cidr_v4_v6_mismatch_does_not_match() {
    let v4: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let cidrs = cidrs(&["::10.0.0.0/120"]); // v6 CIDR for v4 addr
    // v4 vs v6 → no match (mismatch branch returns false)
    assert!(!is_source_allowed(v4, &cidrs));
}

#[test]
fn cidr_multiple_cidrs_any_match_allows() {
    let ip: std::net::IpAddr = "172.16.0.5".parse().unwrap();
    let cidrs = cidrs(&["10.0.0.0/8", "172.16.0.0/16"]);
    assert!(is_source_allowed(ip, &cidrs));
}

#[test]
fn cidr_malformed_entry_is_rejected_during_startup_parse() {
    assert!(parse_allowed_cidrs(&["not-a-cidr".to_string()]).is_err());
}

#[test]
fn cidr_prefix_len_too_large_is_rejected_during_startup_parse() {
    assert!(parse_allowed_cidrs(&["10.0.0.0/33".to_string()]).is_err());
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
