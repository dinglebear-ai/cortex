use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::{ParsedTranscriptRecord, record_key_from_line};

pub fn parse_line(
    line: &str,
    path: &Path,
    line_no: usize,
) -> Result<Option<ParsedTranscriptRecord>> {
    let value: Value = serde_json::from_str(line)?;
    let message = extract_message(&value);
    if message.is_empty() {
        return Ok(None);
    }
    let session_id = value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .or_else(|| value.pointer("/session/id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| Some(path.to_string_lossy().to_string()));
    Ok(Some(ParsedTranscriptRecord {
        record_key: record_key_from_line(&value, line, line_no),
        timestamp: value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        message,
        session_id,
        ai_project: extract_project(&value),
        raw_value: Some(value),
    }))
}

fn extract_project(value: &Value) -> Option<String> {
    value
        .get("cwd")
        .or_else(|| value.get("projectPath"))
        .or_else(|| value.get("project_path"))
        .or_else(|| value.pointer("/message/cwd"))
        .or_else(|| value.pointer("/message/projectPath"))
        .or_else(|| value.pointer("/message/project_path"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn extract_message(value: &Value) -> String {
    if let Some(content) = value.get("content").and_then(Value::as_str) {
        return content.to_string();
    }
    if let Some(message) = value.get("message").and_then(Value::as_str) {
        return message.to_string();
    }
    if let Some(text) = value.pointer("/message/content").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(items) = value.get("content").and_then(Value::as_array)
        && let Some(content) = join_content_items(items)
    {
        return content;
    }
    if let Some(items) = value.pointer("/message/content").and_then(Value::as_array)
        && let Some(content) = join_content_items(items)
    {
        return content;
    }
    String::new()
}

fn join_content_items(items: &[Value]) -> Option<String> {
    let parts: Vec<String> = items
        .iter()
        .filter_map(|item| {
            if let Some(text) = item
                .as_str()
                .or_else(|| item.get("text").and_then(Value::as_str))
                .or_else(|| item.get("content").and_then(Value::as_str))
            {
                return Some(text.to_string());
            }
            // `tool_use` content items (assistant tool calls) carry no
            // `.text`/`.content` string field, so without this branch
            // `extract_message` returns empty and `parse_line` drops the
            // row entirely — before this fix, Claude tool-call rows never
            // reached `logs` at all, which meant MCP event extraction
            // (GH #104) had nothing to extract from. A short synthetic
            // summary keeps the row non-empty and human-readable; the full
            // structured input is separately available via `raw_value` for
            // MCP event extraction, so this summary is not the only copy
            // of the call data.
            if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
                return Some(format!("[tool_use {name}]"));
            }
            None
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
