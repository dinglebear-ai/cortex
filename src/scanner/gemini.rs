//! Gemini CLI transcript parsing.
//!
//! Unlike Claude/Codex (which write line-delimited JSONL), the Gemini CLI
//! stores each session as a single JSON object at `chats/session-*.json` with a
//! top-level `messages` array. The live entry point is therefore [`parse_file`]
//! (whole-file JSON), invoked from `index_gemini_file` in the parent module —
//! Gemini sessions never flow through the per-line `parse_line_for_source`
//! dispatch, so this module carries its own error/observability accounting via
//! [`GeminiParse`] rather than the line loop's `record_parse_error` machinery.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::{ParsedTranscriptRecord, hash_text};

/// Outcome of parsing one Gemini session file.
///
/// Carries the observability signals the whole-file path would otherwise lose:
/// how many messages were skipped for lack of extractable text, and whether the
/// file is a chat file with no `messages` array at all (a likely upstream schema
/// change that must not be silently checkpointed as "fully indexed").
pub struct GeminiParse {
    pub records: Vec<ParsedTranscriptRecord>,
    pub skipped_empty: usize,
    pub missing_messages: bool,
}

pub fn is_chat_file(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|name| name.to_str());
    let parent_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    matches!(parent_name, Some("chats"))
        && file_name.is_some_and(|name| name.starts_with("session-") && name.ends_with(".json"))
}

pub fn parse_file(raw: &str, path: &Path) -> Result<GeminiParse> {
    let value: Value = serde_json::from_str(raw)?;
    let session_id = value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(ToString::to_string)
        });
    let ai_project = value
        .get("cwd")
        .or_else(|| value.get("projectPath"))
        .or_else(|| value.get("project_path"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            value
                .get("projectHash")
                .or_else(|| value.get("project_hash"))
                .and_then(Value::as_str)
                .map(|hash| format!("gemini://project/{hash}"))
        });
    let default_timestamp = value
        .get("startTime")
        .or_else(|| value.get("started_at"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return Ok(GeminiParse {
            records: Vec::new(),
            skipped_empty: 0,
            missing_messages: true,
        });
    };
    let mut records = Vec::new();
    let mut skipped_empty = 0usize;
    for (index, message) in messages.iter().enumerate() {
        let Some(content) = extract_message(message) else {
            skipped_empty += 1;
            continue;
        };
        let serialized = serde_json::to_string(message)?;
        let record_key = message
            .get("id")
            .or_else(|| message.get("uuid"))
            .and_then(Value::as_str)
            .map(|id| format!("id:{id}"))
            .unwrap_or_else(|| format!("message:{index}:hash:{}", hash_text(&serialized)));
        let timestamp = message
            .get("timestamp")
            .or_else(|| message.get("created_at"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| default_timestamp.clone());
        records.push(ParsedTranscriptRecord {
            record_key,
            timestamp,
            message: content,
            session_id: session_id.clone(),
            ai_project: ai_project.clone(),
            event_kind: super::transcript_event_kind(message),
            raw_value: None,
        });
    }
    Ok(GeminiParse {
        records,
        skipped_empty,
        missing_messages: false,
    })
}

/// Extract the textual content of one Gemini message.
///
/// Handles the shapes the Gemini CLI chat log emits: a scalar `content` or
/// `message` string, or a `content` array of strings / `{text}` / `{content}`
/// parts joined by spaces. Returns `None` when no non-empty text can be
/// extracted (an empty turn or an unrecognized shape) so the caller can count
/// the skip rather than silently swallowing it.
fn extract_message(value: &Value) -> Option<String> {
    let text = if let Some(content) = value.get("content").and_then(Value::as_str) {
        content.to_string()
    } else if let Some(message) = value.get("message").and_then(Value::as_str) {
        message.to_string()
    } else {
        let items = value.get("content").and_then(Value::as_array)?;
        items
            .iter()
            .filter_map(|item| {
                item.as_str()
                    .or_else(|| item.get("text").and_then(Value::as_str))
                    .or_else(|| item.get("content").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
#[path = "gemini_tests.rs"]
mod tests;
