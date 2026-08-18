//! Merge `ParserOutput` onto a `LogBatchEntry`.

use crate::db::LogBatchEntry;
use crate::enrich::ParserOutput;
use serde_json::{Value, json};

const PARSE_ERROR_MAX_BYTES: usize = 512;

/// Apply a parser's output to the entry. Caller passes the parser's namespace
/// key so the metadata fields land under the canonical owner.
pub fn merge_output(entry: &mut LogBatchEntry, namespace: &'static str, out: ParserOutput) {
    let mut root: serde_json::Map<String, Value> = entry
        .metadata_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    merge_output_with_metadata(entry, namespace, out, &mut root);
}

/// Merge using metadata that the dispatcher has already parsed. This keeps the
/// ingest hot path to one metadata parse and one serialization per parser run.
pub(crate) fn merge_output_with_metadata(
    entry: &mut LogBatchEntry,
    namespace: &'static str,
    out: ParserOutput,
    root: &mut serde_json::Map<String, Value>,
) {
    if let Some(v) = out.http_status {
        entry.http_status = Some(v);
    }
    if let Some(o) = out.auth_outcome {
        entry.auth_outcome = Some(o.as_str());
    }
    if let Some(v) = out.dns_blocked {
        entry.dns_blocked = Some(v);
    }
    if let Some(v) = out.event_action {
        entry.event_action = Some(v);
    }
    if let Some(s) = out.severity {
        entry.severity = s.to_string();
    }

    if !out.metadata.is_empty() {
        root.insert(namespace.to_string(), Value::Object(out.metadata));
    }
    root.insert(
        "parser".to_string(),
        json!({"name": namespace, "version": 1}),
    );
    entry.metadata_json = Some(Value::Object(std::mem::take(root)).to_string());
}

/// Record a parser failure on the entry. Format: "{parser_name}: {error}",
/// truncated to PARSE_ERROR_MAX_BYTES.
pub fn record_error(entry: &mut LogBatchEntry, parser_name: &str, error: &str) {
    let mut s = format!("{parser_name}: {error}");
    if s.len() > PARSE_ERROR_MAX_BYTES {
        // Back up to a valid UTF-8 char boundary to avoid truncate() panic.
        let mut n = PARSE_ERROR_MAX_BYTES;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
    entry.parse_error = Some(s);
}

/// Stamp `source_kind` into the entry's metadata_json. Called once per ingest
/// path BEFORE the entry reaches the batch writer. Idempotent — if a value is
/// already present, leaves it (caller wins).
pub fn stamp_source_kind(entry: &mut LogBatchEntry, kind: crate::enrich::SourceKind) {
    let mut root: serde_json::Map<String, Value> = match &entry.metadata_json {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::Map::new()),
        None => serde_json::Map::new(),
    };
    if !root.contains_key("source_kind") {
        root.insert(
            "source_kind".to_string(),
            Value::String(kind.as_str().to_string()),
        );
        entry.metadata_json = Some(Value::Object(root).to_string());
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod output_tests;
