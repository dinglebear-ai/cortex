//! OTLP ExportLogsServiceRequest to LogBatchEntry conversion.

use std::net::SocketAddr;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
#[cfg(test)]
use opentelemetry_proto::tonic::common::v1::AnyValue;

use crate::db::LogBatchEntry;
use crate::enrich::{SourceKind, stamp_source_kind};
use crate::ingest_metadata::bounded_metadata_json;

#[cfg(test)]
use super::normalization::{any_value_to_json, attr_key, record_string_table_index_warning};
use super::normalization::{any_value_to_string, normalize_attributes};
#[cfg(test)]
use opentelemetry_proto::tonic::common::v1::any_value::Value as AnyValueKind;

/// Walk the OTLP request and produce one LogBatchEntry per LogRecord.
pub(super) fn build_entries(
    req: &ExportLogsServiceRequest,
    peer: SocketAddr,
) -> Vec<LogBatchEntry> {
    let received_iso = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    let source_ip = peer.to_string();
    let peer_ip = peer.ip().to_string();

    let mut out = Vec::new();
    for resource_logs in &req.resource_logs {
        let resource_attrs = resource_logs
            .resource
            .as_ref()
            .map(|resource| resource.attributes.as_slice())
            .unwrap_or_default();

        for scope_logs in &resource_logs.scope_logs {
            for log in &scope_logs.log_records {
                let normalized = normalize_attributes(resource_attrs, &log.attributes);
                let timestamp = format_otlp_timestamp(log.time_unix_nano)
                    .unwrap_or_else(|| received_iso.clone());
                let severity = severity_from_number(log.severity_number).to_string();
                let message = log
                    .body
                    .as_ref()
                    .and_then(any_value_to_string)
                    .unwrap_or_default();
                let log_ai_tool = normalized.log_ai_tool();
                let metadata_json = bounded_metadata_json(serde_json::json!({
                    "source_type": "otlp",
                    "peer_ip": peer_ip,
                    "peer_port": peer.port(),
                    "host_name": &normalized.host_name,
                    "service_name": &normalized.service_name,
                    "service_version": &normalized.service_version,
                    "severity_number": log.severity_number,
                    "severity_text": log.severity_text,
                    "trace_id": hex_bytes(&log.trace_id),
                    "span_id": hex_bytes(&log.span_id),
                    "flags": log.flags,
                    "event_name": log.event_name,
                    "resource_attributes": &normalized.resource_attributes,
                    "log_attributes": &normalized.legacy_log_signal_attributes,
                }));
                let mut entry = LogBatchEntry {
                    timestamp,
                    hostname: normalized.host_name,
                    facility: Some("otlp".to_string()),
                    severity,
                    app_name: normalized.service_name,
                    process_id: None,
                    message,
                    raw: metadata_json.clone(),
                    source_ip: source_ip.clone(),
                    docker_checkpoint: None,
                    ai_tool: log_ai_tool,
                    ai_project: normalized.ai_project,
                    ai_session_id: normalized.ai_session_id,
                    ai_transcript_path: None,
                    metadata_json: Some(metadata_json),
                    http_status: None,
                    auth_outcome: None,
                    dns_blocked: None,
                    event_action: None,
                    parse_error: None,
                };
                stamp_source_kind(&mut entry, SourceKind::Otlp);
                out.push(entry);
            }
        }
    }
    out
}

fn hex_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn format_otlp_timestamp(time_unix_nano: u64) -> Option<String> {
    if time_unix_nano == 0 {
        return None;
    }
    let secs = (time_unix_nano / 1_000_000_000) as i64;
    let nanos = (time_unix_nano % 1_000_000_000) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// OTLP SeverityNumber (0-24) to syslog severity string.
///
/// 0 (UNSPECIFIED) and any unrecognised value fall through to info rather
/// than dropping the record.
fn severity_from_number(n: i32) -> &'static str {
    match n {
        1..=8 => "debug",
        9..=12 => "info",
        13..=16 => "warning",
        17..=20 => "err",
        21..=24 => "crit",
        _ => "info",
    }
}

#[cfg(test)]
#[path = "entries_tests.rs"]
mod tests;
