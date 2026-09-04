use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::{ParsedTranscriptRecord, TranscriptSessionMetadata, record_key_from_line};

const MAX_SESSION_METADATA_CHARS: usize = 512;

pub fn parse_line(
    line: &str,
    _path: &Path,
    line_no: usize,
) -> Result<Option<ParsedTranscriptRecord>> {
    let value: Value = serde_json::from_str(line)?;
    let message = extract_message(&value);
    let mut session_metadata = extract_session_metadata(&value);
    if message.is_empty() && session_metadata == TranscriptSessionMetadata::default() {
        return Ok(None);
    }
    session_metadata.source_format = Some("codex_rollout_jsonl".to_string());
    let payload = payload(&value);
    let session_id = session_id_from_value(&value);
    let ai_project = extract_project(&value);
    let event_kind = super::transcript_event_kind(&value);
    Ok(Some(ParsedTranscriptRecord {
        record_key: record_key_from_line(&value, line, line_no),
        timestamp: value
            .get("timestamp")
            .or_else(|| payload.get("timestamp"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        message,
        session_id,
        ai_project,
        event_kind,
        session_metadata,
        // `raw_value` is `Some` here (unlike the historical `None`) because
        // MCP event extraction (GH #104) needs the full `payload.arguments`/
        // `payload.output`/`payload.call_id` structure for `function_call`/
        // `function_call_output` rows, which `message` (a short summary,
        // see `extract_message`) does not carry. Codex's skill-tag scanner
        // still reads `message` directly and is unaffected.
        raw_value: Some(value),
    }))
}

fn extract_session_metadata(value: &Value) -> TranscriptSessionMetadata {
    let payload = payload(value);
    let record_type = value.get("type").and_then(Value::as_str);
    let is_session_meta = record_type == Some("session_meta");
    let is_turn_context = record_type == Some("turn_context");

    TranscriptSessionMetadata {
        title: None,
        title_provenance: None,
        agent_name: None,
        model: is_turn_context
            .then(|| metadata_string(payload.get("model")))
            .flatten(),
        model_provider: is_session_meta
            .then(|| metadata_string(payload.get("model_provider")))
            .flatten(),
        client_version: is_session_meta
            .then(|| metadata_string(payload.get("cli_version")))
            .flatten(),
        git_branch: is_session_meta
            .then(|| metadata_string(payload.pointer("/git/branch")))
            .flatten(),
        entrypoint: is_session_meta
            .then(|| metadata_string(payload.get("originator")))
            .flatten(),
        effort: is_turn_context
            .then(|| metadata_string(payload.get("effort")))
            .flatten(),
        source: is_session_meta
            .then(|| metadata_string(payload.get("source")))
            .flatten(),
        thread_source: is_session_meta
            .then(|| metadata_string(payload.get("thread_source")))
            .flatten(),
        source_format: None,
    }
}

fn metadata_string(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.chars().take(MAX_SESSION_METADATA_CHARS).collect())
}

pub fn session_id_from_line(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| session_id_from_value(&value))
}

fn session_id_from_value(value: &Value) -> Option<String> {
    let payload = payload(value);
    value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .or_else(|| value.pointer("/session/id"))
        .or_else(|| payload.get("sessionId"))
        .or_else(|| payload.get("session_id"))
        .or_else(|| payload.pointer("/session/id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            if value.get("type").and_then(Value::as_str) == Some("session_meta") {
                payload
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            } else {
                None
            }
        })
}

fn extract_project(value: &Value) -> Option<String> {
    let payload = payload(value);
    payload
        .get("cwd")
        .or_else(|| value.get("cwd"))
        .or_else(|| value.pointer("/turn_context/cwd"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|args| serde_json::from_str::<Value>(args).ok())
                .and_then(|args| {
                    args.get("workdir")
                        .or_else(|| args.get("cwd"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
        })
}

pub fn project_from_line(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| extract_project(&value))
}

fn payload(value: &Value) -> &Value {
    value.get("payload").unwrap_or(value)
}

fn extract_message(value: &Value) -> String {
    if let Some(text) = value.get("content").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(text) = value.get("message").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(text) = value.pointer("/payload/text").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(text) = value.pointer("/payload/content").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(text) = value
        .pointer("/payload/message/content")
        .and_then(Value::as_str)
    {
        return text.to_string();
    }
    if let Some(items) = value.pointer("/payload/content").and_then(Value::as_array) {
        let joined = join_content_items(items);
        if !joined.is_empty() {
            return joined;
        }
    }
    if let Some(items) = value.pointer("/message/content").and_then(Value::as_array) {
        let joined = join_content_items(items);
        if !joined.is_empty() {
            return joined;
        }
    }
    // `function_call`/`function_call_output` payloads carry no free-text
    // field at all — without this branch `extract_message` returns empty
    // and `parse_line` drops the row entirely, meaning MCP event extraction
    // (GH #104) would have nothing to extract from for Codex tool calls. A
    // short synthetic summary keeps the row non-empty and human-readable;
    // the full structured payload is separately available via `raw_value`.
    if let Some(payload_type) = value.pointer("/payload/type").and_then(Value::as_str) {
        match payload_type {
            "function_call" | "custom_tool_call" => {
                let name = value
                    .pointer("/payload/name")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                return format!("[{payload_type} {name}]");
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = value
                    .pointer("/payload/call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                return format!("[{payload_type} {call_id}]");
            }
            _ => {}
        }
    }
    String::new()
}

fn join_content_items(items: &[Value]) -> String {
    let parts: Vec<&str> = items
        .iter()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("text").and_then(Value::as_str))
                .or_else(|| item.get("content").and_then(Value::as_str))
        })
        .collect();
    parts.join(" ")
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
