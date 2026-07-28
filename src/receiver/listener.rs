use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

use crate::enrich::{SourceKind, stamp_source_kind};
use crate::ingest::IngestTx;

use super::parser::parse_syslog;
use super::writer::source_addr_ip;

#[derive(Debug)]
enum TcpFrame {
    Line(String),
    Oversize { line_bytes: usize, terminated: bool },
    Eof,
}

/// Drain a delimited oversized frame without buffering the excess so the
/// connection can continue at the next frame. A sender that never terminates
/// the frame is cut off once the drain passes this multiple of the max message
/// size, to avoid pinning a TCP slot indefinitely.
///
/// The cutoff is approximate, not an exact byte cap: the budget is only checked
/// on chunks containing no delimiter, so a frame may overrun it by up to one
/// `fill_buf` chunk. Tests should assert the property, not an exact byte count.
const MAX_OVERSIZE_DRAIN_MULTIPLIER: usize = 8;

/// Returns true if `addr` matches any CIDR in `allowed`, or `allowed` is empty
/// (open policy).
///
/// Each entry in `allowed` must be in `<ip>/<prefix_len>` notation. Malformed
/// entries are silently skipped (they were already rejected by
/// `validate_receiver_config` at startup if the config path was used, but env
/// may be set directly in tests or unusual deployments).
fn is_source_allowed(addr: std::net::IpAddr, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    for cidr in allowed {
        if let Some((prefix, len)) = cidr.split_once('/') {
            let Ok(network_addr) = prefix.parse::<std::net::IpAddr>() else {
                continue;
            };
            let Ok(prefix_len) = len.parse::<u32>() else {
                continue;
            };
            if addr_matches_cidr(addr, network_addr, prefix_len) {
                return true;
            }
        }
    }
    false
}

fn addr_matches_cidr(addr: std::net::IpAddr, network: std::net::IpAddr, prefix_len: u32) -> bool {
    match (addr, network) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(n)) => {
            if prefix_len > 32 {
                return false;
            }
            let mask = if prefix_len == 0 {
                0u32
            } else {
                !0u32 << (32 - prefix_len)
            };
            (u32::from(a) & mask) == (u32::from(n) & mask)
        }
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(n)) => {
            if prefix_len > 128 {
                return false;
            }
            let a = u128::from(a);
            let n = u128::from(n);
            let mask = if prefix_len == 0 {
                0u128
            } else {
                !0u128 << (128 - prefix_len)
            };
            (a & mask) == (n & mask)
        }
        _ => false, // v4 vs v6 mismatch
    }
}

/// Read the CIDR allowlist from the environment at listener startup.
///
/// The `ReceiverConfig` struct stores `allowed_source_cidrs` and the field is
/// populated via `env_override_list("CORTEX_ALLOWED_SOURCE_CIDRS", …)` in
/// `Config::load_inner`. The listeners cannot accept the full `ReceiverConfig`
/// because their call sites are in `src/syslog.rs`, which is outside the
/// allowed edit scope for this change. To avoid touching that file, each
/// listener reads the same env var once at startup. When the config path is
/// used the env var will already be set (or the config value was read from
/// TOML), so the two sources stay consistent; tests that set the env var
/// directly also work as expected.
fn load_allowed_cidrs_from_env() -> Vec<String> {
    match std::env::var("CORTEX_ALLOWED_SOURCE_CIDRS") {
        Ok(v) if !v.is_empty() => v
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// UDP syslog receiver.
pub(super) async fn udp_listener(bind: &str, max_size: usize, ingest: IngestTx) -> Result<()> {
    let socket = UdpSocket::bind(bind).await?;
    info!(bind = %bind, "UDP syslog listener bound");

    // Read CIDR allowlist once at startup (empty = accept all).
    let allowed_cidrs = load_allowed_cidrs_from_env();
    if !allowed_cidrs.is_empty() {
        info!(
            cidrs = ?allowed_cidrs,
            "UDP syslog listener: source CIDR allowlist active"
        );
    }

    let mut buf = vec![0u8; max_size];
    let mut backpressure = false;
    let mut received_packets: u64 = 0;
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, addr)) => {
                received_packets += 1;

                // CIDR allowlist check — silently drop packets from unknown sources.
                if !is_source_allowed(addr.ip(), &allowed_cidrs) {
                    debug!(
                        src = %addr,
                        "UDP packet dropped — source not in allowed_source_cidrs"
                    );
                    continue;
                }

                ingest.observability().record_udp_packet(len);
                let raw = String::from_utf8_lossy(&buf[..len]).to_string();
                debug!(
                    src = %addr,
                    len,
                    packet_index = received_packets,
                    queue_depth = ingest.queue_depth(),
                    "UDP syslog packet received"
                );

                match update_backpressure(&mut backpressure, ingest.capacity() == 0) {
                    Some(BackpressureTransition::Applied) => {
                        ingest
                            .observability()
                            .record_write_channel_full_transition();
                        warn!(
                            src = %addr,
                            queue_depth = ingest.queue_depth(),
                            channel_capacity = ingest.queue_capacity(),
                            "syslog write channel full — backpressure applied"
                        );
                    }
                    Some(BackpressureTransition::Cleared) => {
                        info!(
                            src = %addr,
                            queue_depth = ingest.queue_depth(),
                            channel_capacity = ingest.queue_capacity(),
                            "syslog write channel cleared — backpressure lifted"
                        );
                    }
                    None => {}
                }

                let mut entry = parse_syslog(&raw, addr.to_string());
                stamp_source_kind(&mut entry, SourceKind::SyslogUdp);
                match ingest.try_send(entry) {
                    Ok(()) => {}
                    Err(crate::ingest::TrySendErr::Full) => {
                        // Packet dropped; channel backpressure already logged above.
                        // try_send is used (not .await) so the UDP recv loop is never
                        // blocked — kernel buffer absorbs bursts, explicit drop counter
                        // is tracked via observability.record_enqueue_error.
                        ingest
                            .observability()
                            .record_udp_packet_dropped_queue_full(ingest.queue_depth());
                    }
                    Err(crate::ingest::TrySendErr::Closed) => {
                        error!("Write channel closed");
                        break;
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "UDP recv error");
            }
        }
    }
    Ok(())
}

/// Per-connection handler for TCP syslog streams.
pub(super) async fn handle_tcp_connection(
    stream: tokio::net::TcpStream,
    addr: std::net::SocketAddr,
    ingest: IngestTx,
    max_size: usize,
    idle_timeout_secs: u64,
    allowed_cidrs: &[String],
) {
    // CIDR allowlist check — reject connections from unknown sources early.
    if !is_source_allowed(addr.ip(), allowed_cidrs) {
        debug!(
            peer = %addr,
            "TCP connection dropped — source not in allowed_source_cidrs"
        );
        return;
    }

    let observability = ingest.observability();
    observability.record_tcp_connection_accepted();
    info!(peer = %addr, "TCP syslog connection accepted");
    // Persistent forwarders like rsyslog reuse a single TCP session for many
    // syslog frames, so max_size must apply per message line, not to the whole
    // connection lifetime.
    let mut reader = BufReader::new(stream);
    let mut backpressure = false;
    let mut line_count: u64 = 0;
    let mut total_bytes: usize = 0;
    let mut oversize_count: u64 = 0;
    let mut oversize_bytes_total: usize = 0;
    let mut peer_hostname: Option<String> = None;
    let started = Instant::now();
    let close_reason = loop {
        // Idle timeout is per read, not wall-clock lifetime.
        let next = tokio::time::timeout(
            tokio::time::Duration::from_secs(idle_timeout_secs),
            read_bounded_line(&mut reader, max_size),
        );
        match next.await {
            Ok(Ok(TcpFrame::Line(line))) => {
                if line.is_empty() {
                    continue;
                }
                line_count += 1;
                total_bytes += line.len();
                observability.record_tcp_line(line.len());

                match update_backpressure(&mut backpressure, ingest.capacity() == 0) {
                    Some(BackpressureTransition::Applied) => {
                        observability.record_write_channel_full_transition();
                        warn!(
                            peer = %addr,
                            queue_depth = ingest.queue_depth(),
                            channel_capacity = ingest.queue_capacity(),
                            line_count,
                            "syslog write channel full — backpressure applied"
                        );
                    }
                    Some(BackpressureTransition::Cleared) => {
                        info!(
                            peer = %addr,
                            queue_depth = ingest.queue_depth(),
                            channel_capacity = ingest.queue_capacity(),
                            line_count,
                            "syslog write channel cleared — backpressure lifted"
                        );
                    }
                    None => {}
                }
                debug!(
                    peer = %addr,
                    line_count,
                    line_bytes = line.len(),
                    queue_depth = ingest.queue_depth(),
                    "TCP syslog line received"
                );
                let mut entry = parse_syslog(&line, addr.to_string());
                stamp_source_kind(&mut entry, SourceKind::SyslogTcp);
                if peer_hostname.is_none() {
                    peer_hostname = Some(entry.hostname.clone());
                    info!(
                        peer = %addr,
                        hostname = %entry.hostname,
                        source_ip = %source_addr_ip(&entry.source_ip),
                        "TCP syslog sender identified"
                    );
                }
                match ingest.try_send(entry) {
                    Ok(()) => {}
                    Err(crate::ingest::TrySendErr::Full) => {
                        observability.record_tcp_line_dropped_queue_full(ingest.queue_depth());
                        // Unlike UDP, TCP is reliable — the sender had no indication
                        // this line was dropped. Emit an explicit warn per TCP drop so
                        // the loss is observable; the batch backpressure log above marks
                        // the window start but doesn't count individual drops.
                        warn!(
                            peer = %addr,
                            line_count,
                            "TCP syslog line dropped — write channel full"
                        );
                    }
                    Err(crate::ingest::TrySendErr::Closed) => {
                        break "write_channel_closed";
                    }
                }
            }
            Ok(Ok(TcpFrame::Oversize {
                line_bytes,
                terminated,
            })) => {
                observability.record_tcp_line_dropped_oversize();
                oversize_count += 1;
                oversize_bytes_total = oversize_bytes_total.saturating_add(line_bytes);
                // A terminated oversize frame no longer tears the connection
                // down, so a misconfigured forwarder can emit these at line
                // rate. Log on an exponential cadence instead of per frame; the
                // per-connection totals ride the closing summary and the exact
                // count stays available via the observability counter.
                if !terminated || should_log_oversize(oversize_count) {
                    warn!(
                        peer = %addr,
                        line_count,
                        line_bytes,
                        oversize_count,
                        max_message_size = max_size,
                        terminated,
                        "Dropping oversized TCP syslog line"
                    );
                }
                if terminated {
                    continue;
                }
                break "oversized_unterminated_line";
            }
            Ok(Ok(TcpFrame::Eof)) => break "eof",
            Ok(Err(e)) => {
                error!(peer = %addr, error = %e, "TCP syslog read error");
                break "read_error";
            }
            Err(_) => {
                warn!(peer = %addr, idle_timeout_secs, "TCP syslog connection timed out");
                break "idle_timeout";
            }
        }
    };
    info!(
        peer = %addr,
        hostname = peer_hostname.as_deref().unwrap_or("unknown"),
        close_reason,
        line_count,
        total_bytes,
        oversize_count,
        oversize_bytes_total,
        elapsed_ms = started.elapsed().as_millis(),
        "TCP syslog connection closed"
    );
    observability.record_tcp_connection_closed();
}

/// Exponential log cadence for repeated oversize drops on one connection:
/// the 1st, 10th, 100th, ... drop is logged, the rest are counted only.
fn should_log_oversize(oversize_count: u64) -> bool {
    let mut threshold = 1u64;
    while threshold < oversize_count {
        threshold = threshold.saturating_mul(10);
        if threshold == u64::MAX {
            return false;
        }
    }
    threshold == oversize_count
}

async fn read_bounded_line<R>(reader: &mut R, max_size: usize) -> std::io::Result<TcpFrame>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::with_capacity(max_size.min(8192));
    let mut oversize_bytes = None;
    // Accumulate one byte past `max_size` so a CRLF terminator split across two
    // `fill_buf` chunks does not misclassify an at-limit payload. A frame of
    // exactly `max_size` payload bytes ending in CRLF must be accepted whether
    // or not the read boundary falls between the `\r` and the `\n`; the newline
    // branch below subtracts the trailing `\r` before comparing to `max_size`.
    let max_accumulate_bytes = max_size.saturating_add(1);
    let max_drain_bytes = max_size
        .saturating_mul(MAX_OVERSIZE_DRAIN_MULTIPLIER)
        .max(max_accumulate_bytes);

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if let Some(line_bytes) = oversize_bytes {
                return Ok(TcpFrame::Oversize {
                    line_bytes,
                    terminated: false,
                });
            }
            return if line.is_empty() {
                Ok(TcpFrame::Eof)
            } else {
                Ok(TcpFrame::Line(decode_tcp_line(&line)))
            };
        }

        if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
            let take = pos + 1;
            if let Some(discarded) = oversize_bytes {
                let total = discarded.saturating_add(take);
                reader.consume(take);
                return Ok(TcpFrame::Oversize {
                    line_bytes: total,
                    terminated: true,
                });
            }

            let total = line.len().saturating_add(take);
            // CRLF can straddle a `fill_buf` boundary: when `\n` opens this
            // chunk the `\r` is the last byte already accumulated in `line`.
            let has_cr = if pos > 0 {
                available[pos - 1] == b'\r'
            } else {
                line.last() == Some(&b'\r')
            };
            let payload_bytes = line
                .len()
                .saturating_add(pos)
                .saturating_sub(usize::from(has_cr));
            if payload_bytes > max_size {
                reader.consume(take);
                return Ok(TcpFrame::Oversize {
                    line_bytes: total,
                    terminated: true,
                });
            }
            line.extend_from_slice(&available[..take]);
            reader.consume(take);
            return Ok(TcpFrame::Line(decode_tcp_line(&line)));
        }

        let available_len = available.len();
        let total = oversize_bytes
            .unwrap_or(line.len())
            .saturating_add(available_len);

        if oversize_bytes.is_some() || total > max_accumulate_bytes {
            reader.consume(available_len);
            // Nothing accumulated so far can be part of a deliverable frame.
            line = Vec::new();
            if total > max_drain_bytes {
                return Ok(TcpFrame::Oversize {
                    line_bytes: total,
                    terminated: false,
                });
            }
            oversize_bytes = Some(total);
            continue;
        }

        line.extend_from_slice(available);
        reader.consume(available_len);
    }
}

fn decode_tcp_line(raw: &[u8]) -> String {
    let mut end = raw.len();
    while end > 0 && matches!(raw[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    String::from_utf8_lossy(&raw[..end]).to_string()
}

/// TCP syslog receiver (newline-delimited).
///
/// Caps concurrent connections at `max_connections` via a semaphore; each
/// connection is subject to an `idle_timeout_secs` idle timeout (per read)
/// to evict zombie connections.
pub(super) async fn tcp_listener(
    bind: &str,
    ingest: IngestTx,
    max_size: usize,
    max_connections: usize,
    idle_timeout_secs: u64,
) -> Result<()> {
    let listener = TcpListener::bind(bind).await?;
    info!(bind = %bind, max_connections, idle_timeout_secs, "TCP syslog listener bound");

    // Read CIDR allowlist once at startup (empty = accept all).
    let allowed_cidrs = Arc::new(load_allowed_cidrs_from_env());
    if !allowed_cidrs.is_empty() {
        info!(
            cidrs = ?allowed_cidrs,
            "TCP syslog listener: source CIDR allowlist active"
        );
    }

    let sem = Arc::new(Semaphore::new(max_connections));
    let mut accept_backoff_ms: u64 = 100;
    let mut reject_logged = false;
    let mut last_reject_log = std::time::Instant::now();
    let mut total_rejected: u64 = 0;

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                accept_backoff_ms = 100;
                match Arc::clone(&sem).try_acquire_owned() {
                    Ok(permit) => {
                        let available_permits = sem.available_permits();
                        let ingest = ingest.clone();
                        let cidrs = Arc::clone(&allowed_cidrs);
                        tokio::spawn(async move {
                            let _permit = permit;
                            handle_tcp_connection(
                                stream,
                                addr,
                                ingest,
                                max_size,
                                idle_timeout_secs,
                                &cidrs,
                            )
                            .await;
                        });
                        debug!(
                            peer = %addr,
                            active_connections = max_connections.saturating_sub(available_permits),
                            max_connections,
                            "TCP syslog connection dispatched"
                        );
                    }
                    Err(tokio::sync::TryAcquireError::NoPermits) => {
                        total_rejected += 1;
                        ingest.observability().record_tcp_connection_rejected();
                        if !reject_logged
                            || last_reject_log.elapsed() >= std::time::Duration::from_secs(10)
                        {
                            warn!(
                                peer = %addr,
                                max_connections,
                                total_rejected,
                                "TCP connection limit reached — rejecting connection"
                            );
                            reject_logged = true;
                            last_reject_log = std::time::Instant::now();
                        }
                    }
                    Err(tokio::sync::TryAcquireError::Closed) => {
                        error!(
                            "TCP connection semaphore unexpectedly closed — TCP listener exiting"
                        );
                        break;
                    }
                }
            }
            Err(e) => {
                error!(error = %e, accept_backoff_ms, "TCP accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(accept_backoff_ms)).await;
                accept_backoff_ms = (accept_backoff_ms * 2).min(5000);
                continue;
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BackpressureTransition {
    Applied,
    Cleared,
}

pub(super) fn update_backpressure(
    backpressure: &mut bool,
    at_capacity: bool,
) -> Option<BackpressureTransition> {
    match (at_capacity, *backpressure) {
        (true, false) => {
            *backpressure = true;
            Some(BackpressureTransition::Applied)
        }
        (false, true) => {
            *backpressure = false;
            Some(BackpressureTransition::Cleared)
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "listener_tests.rs"]
mod tests;
