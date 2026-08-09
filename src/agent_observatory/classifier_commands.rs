//! Agent-command and shell-history classification.

use crate::agent_observatory::identity::canonical_tool;
use crate::db::LogEntry;
use serde_json::{Map, Value};
use std::path::{Component, Path};

pub const MAX_COMMAND_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_COMMAND_METADATA_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandLogSource {
    AgentCommand,
    Atuin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSkipReason {
    InvalidLogId,
    MissingHostname,
    UnsupportedSource,
    SourceShapeMismatch,
    MissingTool,
    MissingProviderSession,
    MissingShellSession,
    MissingCwd,
    NonCanonicalCwd,
    InconsistentCanonicalFields,
    UnsupportedShell,
    InvalidTimestamp,
    InvalidReceivedAt,
    InvalidFinishedAt,
    InvalidExitStatus,
    InvalidDuration,
    MetadataTooLarge,
    InvalidMetadataJson,
    MetadataNotObject,
    ContentNotScrubbed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSkipDiagnostic {
    pub log_id: i64,
    pub reason: CommandSkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLogProjection {
    pub log_id: i64,
    pub timestamp: String,
    pub received_at: String,
    pub hostname: String,
    pub source: CommandLogSource,
    pub tool: Option<String>,
    pub provider_tool: Option<String>,
    pub provider_session_id: Option<String>,
    pub shell_session_id: Option<String>,
    pub cwd: String,
    pub process_id: Option<String>,
    pub severity: String,
    pub command: String,
    pub command_truncated: bool,
    pub exit_status: Option<i32>,
    pub duration_ms: Option<u64>,
    pub finished_at: Option<String>,
    pub command_surface: Option<String>,
    pub content_scrubbed: bool,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandLogClassification {
    Project(Box<CommandLogProjection>),
    Skip(CommandSkipDiagnostic),
}

fn skip(log_id: i64, reason: CommandSkipReason) -> CommandLogClassification {
    CommandLogClassification::Skip(CommandSkipDiagnostic { log_id, reason })
}

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn canonical_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}

fn consistent(primary: Option<&str>, metadata: &str) -> bool {
    primary
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(|value| value == metadata)
}

fn parse_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

fn optional_i32(value: Option<&Value>) -> Result<Option<i32>, CommandSkipReason> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or(CommandSkipReason::InvalidExitStatus),
    }
}

fn optional_u64(value: Option<&Value>) -> Result<Option<u64>, CommandSkipReason> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or(CommandSkipReason::InvalidDuration),
    }
}

fn metadata_object(row: &LogEntry) -> Result<(String, Map<String, Value>), CommandSkipReason> {
    let Some(metadata) = row.metadata_json.as_deref() else {
        return Err(CommandSkipReason::SourceShapeMismatch);
    };
    if metadata.len() > MAX_COMMAND_METADATA_BYTES {
        return Err(CommandSkipReason::MetadataTooLarge);
    }
    let value: Value =
        serde_json::from_str(metadata).map_err(|_| CommandSkipReason::InvalidMetadataJson)?;
    let Value::Object(object) = value else {
        return Err(CommandSkipReason::MetadataNotObject);
    };
    if object.get("content_scrubbed").and_then(Value::as_bool) != Some(true) {
        return Err(CommandSkipReason::ContentNotScrubbed);
    }
    Ok((metadata.to_string(), object))
}

fn classify_agent_command(
    row: &LogEntry,
    metadata_json: String,
    root: &Map<String, Value>,
) -> Result<CommandLogProjection, CommandSkipReason> {
    if text(root.get("source_type")) != Some("agent_command") {
        return Err(CommandSkipReason::SourceShapeMismatch);
    }
    let Some(Value::Object(command)) = root.get("agent_command") else {
        return Err(CommandSkipReason::SourceShapeMismatch);
    };
    let provider_tool = text(command.get("agent"))
        .or_else(|| {
            row.ai_tool
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or(CommandSkipReason::MissingTool)?;
    let tool = canonical_tool(provider_tool).map_err(|_| CommandSkipReason::MissingTool)?;
    let provider_session_id = text(command.get("session_id"))
        .or_else(|| {
            row.ai_session_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or(CommandSkipReason::MissingProviderSession)?;
    let cwd = text(command.get("cwd"))
        .or_else(|| {
            row.ai_project
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or(CommandSkipReason::MissingCwd)?;
    if !canonical_path(cwd) {
        return Err(CommandSkipReason::NonCanonicalCwd);
    }
    if !consistent(row.ai_tool.as_deref(), provider_tool)
        || !consistent(row.ai_session_id.as_deref(), provider_session_id)
        || !consistent(row.ai_project.as_deref(), cwd)
    {
        return Err(CommandSkipReason::InconsistentCanonicalFields);
    }
    let finished_at =
        text(command.get("finished_at")).ok_or(CommandSkipReason::InvalidFinishedAt)?;
    let finished = chrono::DateTime::parse_from_rfc3339(finished_at)
        .map_err(|_| CommandSkipReason::InvalidFinishedAt)?;
    let started = chrono::DateTime::parse_from_rfc3339(&row.timestamp)
        .map_err(|_| CommandSkipReason::InvalidTimestamp)?;
    if finished < started {
        return Err(CommandSkipReason::InvalidFinishedAt);
    }
    let duration_ms = optional_u64(command.get("duration_ms"))?;
    let exit_status = optional_i32(command.get("exit_status"))?;
    let (command_text, command_truncated) = truncate_utf8(&row.message, MAX_COMMAND_MESSAGE_BYTES);
    Ok(CommandLogProjection {
        log_id: row.id,
        timestamp: row.timestamp.clone(),
        received_at: row.received_at.clone(),
        hostname: row.hostname.trim().to_string(),
        source: CommandLogSource::AgentCommand,
        tool: Some(tool),
        provider_tool: Some(provider_tool.to_string()),
        provider_session_id: Some(provider_session_id.to_string()),
        shell_session_id: None,
        cwd: cwd.to_string(),
        process_id: row.process_id.clone(),
        severity: row.severity.trim().to_string(),
        command: command_text,
        command_truncated,
        exit_status,
        duration_ms,
        finished_at: Some(finished_at.to_string()),
        command_surface: text(command.get("command_surface")).map(str::to_string),
        content_scrubbed: true,
        metadata_json,
    })
}

fn shell_object(root: &Map<String, Value>) -> Option<(&str, Option<&Map<String, Value>>)> {
    match root.get("shell") {
        Some(Value::Object(shell)) => text(shell.get("name")).map(|name| (name, Some(shell))),
        Some(Value::String(name)) if !name.trim().is_empty() => Some((name.trim(), None)),
        _ => None,
    }
}

fn classify_atuin(
    row: &LogEntry,
    metadata_json: String,
    root: &Map<String, Value>,
) -> Result<CommandLogProjection, CommandSkipReason> {
    if text(root.get("source_type")) != Some("shell_history") {
        return Err(CommandSkipReason::SourceShapeMismatch);
    }
    let (shell_name, nested) = shell_object(root).ok_or(CommandSkipReason::SourceShapeMismatch)?;
    if shell_name.to_lowercase() != "atuin" {
        return Err(CommandSkipReason::UnsupportedShell);
    }
    let nested_value = |name| nested.and_then(|shell| shell.get(name));
    let cwd = text(nested_value("cwd"))
        .or_else(|| text(root.get("cwd")))
        .or_else(|| {
            row.ai_project
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or(CommandSkipReason::MissingCwd)?;
    let shell_session_id = text(nested_value("session"))
        .or_else(|| text(root.get("session_id")))
        .or_else(|| {
            row.ai_session_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or(CommandSkipReason::MissingShellSession)?;
    if !canonical_path(cwd) {
        return Err(CommandSkipReason::NonCanonicalCwd);
    }
    if !consistent(row.ai_project.as_deref(), cwd)
        || !consistent(row.ai_session_id.as_deref(), shell_session_id)
    {
        return Err(CommandSkipReason::InconsistentCanonicalFields);
    }
    let exit_status =
        optional_i32(nested_value("exit_status").or_else(|| root.get("exit_status")))?;
    let duration_ms =
        optional_u64(nested_value("duration_ms").or_else(|| root.get("duration_ms")))?;
    let (command_text, command_truncated) = truncate_utf8(&row.message, MAX_COMMAND_MESSAGE_BYTES);
    Ok(CommandLogProjection {
        log_id: row.id,
        timestamp: row.timestamp.clone(),
        received_at: row.received_at.clone(),
        hostname: row.hostname.trim().to_string(),
        source: CommandLogSource::Atuin,
        tool: None,
        provider_tool: None,
        provider_session_id: None,
        shell_session_id: Some(shell_session_id.to_string()),
        cwd: cwd.to_string(),
        process_id: row.process_id.clone(),
        severity: row.severity.trim().to_string(),
        command: command_text,
        command_truncated,
        exit_status,
        duration_ms,
        finished_at: None,
        command_surface: Some("shell_history".to_string()),
        content_scrubbed: true,
        metadata_json,
    })
}

pub fn classify_command_log(row: &LogEntry) -> CommandLogClassification {
    if row.id <= 0 {
        return skip(row.id, CommandSkipReason::InvalidLogId);
    }
    if row.hostname.trim().is_empty() {
        return skip(row.id, CommandSkipReason::MissingHostname);
    }
    if !parse_timestamp(&row.timestamp) {
        return skip(row.id, CommandSkipReason::InvalidTimestamp);
    }
    if !parse_timestamp(&row.received_at) {
        return skip(row.id, CommandSkipReason::InvalidReceivedAt);
    }
    let (metadata_json, root) = match metadata_object(row) {
        Ok(value) => value,
        Err(reason) => return skip(row.id, reason),
    };
    let result = if row.source_ip.starts_with("agent-command://") {
        classify_agent_command(row, metadata_json, &root)
    } else if row.source_ip.starts_with("shell-history://")
        || row.source_ip.starts_with("agent-shell-history://")
    {
        classify_atuin(row, metadata_json, &root)
    } else {
        return skip(row.id, CommandSkipReason::UnsupportedSource);
    };
    match result {
        Ok(projected) => CommandLogClassification::Project(Box::new(projected)),
        Err(reason) => skip(row.id, reason),
    }
}

#[cfg(test)]
#[path = "classifier_commands_tests.rs"]
mod tests;
