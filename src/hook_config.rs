//! Hook CONFIG-INVENTORY collectors: read local host hook configuration and
//! trust-state files into `ExtractedHookEvent`s carrying
//! `evidence_kind = config_inventory` (configured hooks) or
//! `trusted_hash_state` (Codex trusted hashes). These run against LOCAL FILES
//! on the host running cortex, NOT transcript content — they are a distinct,
//! clearly-scoped collector separate from the transcript runtime extractor in
//! `crate::scanner::hook_events`.
//!
//! CRITICAL (GH #105 acceptance criterion): a configured hook is NOT proof it
//! executed. Every event produced here carries a config/trust `evidence_kind`
//! and `status = "configured"`, never a runtime status, and `ai_session_id =
//! None`. Callers use config inventory only to detect `hook_not_invoked` by
//! comparing against runtime evidence for the SAME session — never as a
//! standalone claim of non-execution.
//!
//! Sources:
//! - Claude: `~/.claude/settings.json` `hooks` object (config_inventory).
//! - Codex: `~/.codex/hooks.json` configured hook groups (config_inventory).
//! - Codex: `~/.codex/config.toml` `[hooks.state]` trusted hashes / source
//!   keys (trusted_hash_state).

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::scanner::hook_events::{ExtractedHookEvent, HookEvidenceKind, HookStatus};

/// One collected config-inventory event plus the host/timestamp context the
/// caller needs to build a `HookEventInsert`. `ai_session_id` is intentionally
/// absent — config rows are host-global, not session-scoped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedHookConfig {
    pub ai_tool: String,
    pub hostname: String,
    pub timestamp: String,
    pub event: ExtractedHookEvent,
}

/// Resolve the home directory the same way the transcript-root resolver does
/// (`$HOME`), so the collector reads the same account cortex ingests
/// transcripts for.
fn home_dir() -> PathBuf {
    crate::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(""))
}

/// Collect all local hook config inventory + trust-state for the given host and
/// timestamp. Missing files are skipped silently (returns fewer rows), parse
/// errors on an individual file are skipped without failing the whole collect.
pub fn collect_hook_config(hostname: &str, timestamp: &str) -> Vec<CollectedHookConfig> {
    let home = home_dir();
    let mut out = Vec::new();
    collect_claude_settings(
        &home.join(".claude/settings.json"),
        hostname,
        timestamp,
        &mut out,
    );
    collect_codex_hooks(
        &home.join(".codex/hooks.json"),
        hostname,
        timestamp,
        &mut out,
    );
    collect_codex_trust_state(
        &home.join(".codex/config.toml"),
        hostname,
        timestamp,
        &mut out,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn push_event(
    out: &mut Vec<CollectedHookConfig>,
    ai_tool: &str,
    hostname: &str,
    timestamp: &str,
    hook_event: String,
    hook_name: Option<String>,
    hook_source: Option<String>,
    hook_command: Option<String>,
    trusted_hash: Option<String>,
    evidence_kind: HookEvidenceKind,
) {
    let event = ExtractedHookEvent {
        hook_event,
        hook_name,
        hook_source,
        hook_command,
        // Config/trust rows carry the dedicated `Configured` status, never a
        // runtime status — this is what keeps them out of runtime failure
        // anchors and out of "the hook executed" claims.
        status: HookStatus::Configured,
        exit_code: None,
        duration_ms: None,
        stdout_preview: None,
        stderr_preview: None,
        persisted_output_path: None,
        trusted_hash,
        evidence_kind,
        metadata_json: None,
    };
    if let Some(normalized) = event.normalized() {
        out.push(CollectedHookConfig {
            ai_tool: ai_tool.to_string(),
            hostname: hostname.to_string(),
            timestamp: timestamp.to_string(),
            event: normalized,
        });
    }
}

/// Parse `~/.claude/settings.json` `hooks` object into config_inventory rows.
/// Claude's settings shape is `{ "hooks": { "<EventName>": [ { "matcher": ...,
/// "hooks": [ { "type": "command", "command": "..." }, ... ] }, ... ] } }`.
fn collect_claude_settings(
    path: &Path,
    hostname: &str,
    timestamp: &str,
    out: &mut Vec<CollectedHookConfig>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(hooks) = value.get("hooks").and_then(|v| v.as_object()) else {
        return;
    };
    for (event_name, matchers) in hooks {
        let Some(matcher_arr) = matchers.as_array() else {
            continue;
        };
        for matcher in matcher_arr {
            let matcher_str = matcher
                .get("matcher")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let Some(hook_list) = matcher.get("hooks").and_then(|v| v.as_array()) else {
                continue;
            };
            for hook in hook_list {
                let command = hook
                    .get("command")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                push_event(
                    out,
                    "claude",
                    hostname,
                    timestamp,
                    event_name.clone(),
                    matcher_str.clone(),
                    Some("claude:settings.json".to_string()),
                    command,
                    None,
                    HookEvidenceKind::ConfigInventory,
                );
            }
        }
    }
}

/// Parse `~/.codex/hooks.json` configured hook groups into config_inventory
/// rows. Codex's shape is `{ "<GroupName>": [ { "command": [...] | "...", ...
/// }, ... ] }` where GroupName is one of Stop/SessionStart/UserPromptSubmit/
/// PreToolUse/PostToolUse/PermissionRequest/PreCompact/PostCompact.
fn collect_codex_hooks(
    path: &Path,
    hostname: &str,
    timestamp: &str,
    out: &mut Vec<CollectedHookConfig>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    // Accept either a top-level object of groups, or `{ "hooks": { ... } }`.
    let groups = value
        .get("hooks")
        .and_then(|v| v.as_object())
        .or_else(|| value.as_object());
    let Some(groups) = groups else {
        return;
    };
    for (group_name, entries) in groups {
        let Some(arr) = entries.as_array() else {
            continue;
        };
        for entry in arr {
            let command = codex_command_string(entry);
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            push_event(
                out,
                "codex",
                hostname,
                timestamp,
                group_name.clone(),
                name,
                Some("codex:hooks.json".to_string()),
                command,
                None,
                HookEvidenceKind::ConfigInventory,
            );
        }
    }
}

/// Codex hook `command` can be a string or an array of argv tokens. Render
/// either into a single bounded command string (the shared `normalized()`
/// clamps it).
fn codex_command_string(entry: &serde_json::Value) -> Option<String> {
    match entry.get("command") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(arr)) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect();
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        _ => None,
    }
}

/// Parse `~/.codex/config.toml` `[hooks.state]` trusted hashes / source keys
/// into trusted_hash_state rows. The `[hooks.state]` table maps a hook source
/// key to a trusted hash (and possibly other trust metadata).
fn collect_codex_trust_state(
    path: &Path,
    hostname: &str,
    timestamp: &str,
    out: &mut Vec<CollectedHookConfig>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return;
    };
    let Some(state) = value
        .get("hooks")
        .and_then(|h| h.get("state"))
        .and_then(|s| s.as_table())
    else {
        return;
    };
    for (source_key, entry) in state {
        let trusted_hash = match entry {
            toml::Value::String(s) => Some(s.clone()),
            toml::Value::Table(t) => t
                .get("trusted_hash")
                .or_else(|| t.get("hash"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string),
            _ => None,
        };
        push_event(
            out,
            "codex",
            hostname,
            timestamp,
            // `[hooks.state]` is keyed by source, not by hook event; use a
            // stable synthetic event name so these rows group together and
            // never collide with a real runtime hook_event.
            "hook_trust_state".to_string(),
            Some(source_key.clone()),
            Some("codex:config.toml[hooks.state]".to_string()),
            None,
            trusted_hash,
            HookEvidenceKind::TrustedHashState,
        );
    }
}

/// Collect config inventory for the local host and persist it. Returns the
/// number of newly-inserted rows. Idempotent across repeated runs at the same
/// `timestamp` via the `ai_hook_events` UNIQUE constraint.
pub fn collect_and_store(
    pool: &crate::db::DbPool,
    hostname: &str,
    timestamp: &str,
) -> Result<usize> {
    let collected = collect_hook_config(hostname, timestamp);
    if collected.is_empty() {
        return Ok(0);
    }
    let inserts: Vec<crate::db::HookEventInsert> = collected
        .into_iter()
        .map(|c| crate::db::HookEventInsert {
            log_id: None,
            ai_tool: c.ai_tool,
            ai_project: None,
            ai_session_id: None,
            hostname: c.hostname,
            timestamp: c.timestamp,
            event: c.event,
        })
        .collect();
    crate::db::insert_hook_events(pool, &inserts)
}

#[cfg(test)]
#[path = "hook_config_tests.rs"]
mod tests;
