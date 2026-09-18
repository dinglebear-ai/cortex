//! Skill-event extraction from AI transcript records.
//!
//! Two independent extractors feed the same [`ExtractedSkillEvent`] shape:
//! - Claude: structured `attributionSkill` / `attributionPlugin` JSON fields
//!   plus observed package-qualified skill command envelopes such as
//!   `<command-message>vibin:repo-status</command-message>` and
//!   `<command-name>/vibin:repo-status</command-name>`.
//! - Codex: native skill headers in transcript content, including truncated
//!   bodies, and separately typed successful command-read evidence. A JSON
//!   marker in ordinary message text is never command-execution evidence.
//!
//! Both extractors short-circuit on a cheap substring check before doing any
//! real parsing/regex work (eng review Fix 1), so the common no-skill-event
//! case costs a single `str::contains` call.
//!
//! Callers normalize with [`ExtractedSkillEvent::normalized`] before
//! inserting, which trims/clamps/derives the `plugin:skill` combined form
//! and rejects control characters (eng review Fix 8 — an adversarial
//! transcript could otherwise embed ANSI escapes that the CLI printer
//! would echo verbatim via `println!`).

use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest, Sha256};

const MAX_SKILL_FIELD_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillEventKind {
    ClaudeAttribution,
    ClaudeSkillCommand,
    CodexSkillBlock,
    CodexSkillRead,
}

impl SkillEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeAttribution => "claude_attribution",
            Self::ClaudeSkillCommand => "claude_skill_command",
            Self::CodexSkillBlock => "codex_skill_block",
            Self::CodexSkillRead => "codex_skill_read",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillEvidenceKind {
    StructuredJsonField,
    TranscriptContent,
}

impl SkillEvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StructuredJsonField => "structured_json_field",
            Self::TranscriptContent => "transcript_content",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSkillEvent {
    pub skill_name: String,
    pub skill_plugin: Option<String>,
    pub event_kind: SkillEventKind,
    pub evidence_kind: SkillEvidenceKind,
}

impl ExtractedSkillEvent {
    /// Trim, reject-if-empty, reject-if-contains-control-characters, and
    /// clamp `skill_name`/`skill_plugin` to `MAX_SKILL_FIELD_CHARS`. Returns
    /// `None` when the resulting skill_name would be empty OR contains any
    /// `char::is_control()` character (eng review Fix 8 — closes a terminal
    /// output spoofing vector: ANSI escapes or embedded newlines/CRs in a
    /// skill name would otherwise be echoed verbatim by the CLI's
    /// `println!`-based printer). Never panics or bubbles an error —
    /// callers skip the event and keep parsing the rest of the transcript.
    fn normalized(mut self) -> Option<Self> {
        let trimmed_name = self.skill_name.trim();
        if trimmed_name.is_empty() || trimmed_name.chars().any(char::is_control) {
            return None;
        }
        if self
            .skill_plugin
            .as_deref()
            .is_some_and(|plugin| plugin.chars().any(char::is_control))
        {
            return None;
        }
        // If the source already used "plugin:skill" combined form, split it
        // out for skill_plugin while keeping skill_name as the full combined
        // string (locked behavior — do not fabricate this split when
        // plugin/skill came from separate source fields, e.g. Claude's
        // attributionSkill/attributionPlugin).
        if self.skill_plugin.is_none()
            && let Some((plugin, _rest)) = trimmed_name.split_once(':')
            && !plugin.is_empty()
        {
            self.skill_plugin = Some(plugin.to_string());
        }
        self.skill_name = clamp_chars(trimmed_name, MAX_SKILL_FIELD_CHARS);
        self.skill_plugin = self.skill_plugin.and_then(|plugin| {
            let trimmed = plugin.trim();
            (!trimmed.is_empty()).then(|| clamp_chars(trimmed, MAX_SKILL_FIELD_CHARS))
        });
        Some(self)
    }
}

fn clamp_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        value.chars().take(max_chars).collect()
    }
}

/// Cheap raw-line guard shared by live ingest and historical backfill.
/// Modern Claude desktop sessions do not consistently emit `attributionSkill`;
/// package-qualified skill invocations are instead represented by command
/// envelopes in the user message.
pub(crate) fn claude_line_may_contain_skill_event(line: &str) -> bool {
    line.contains("attributionSkill")
        || line.contains("<command-message>")
        || line.contains("<command-name>")
}

static CLAUDE_SKILL_COMMAND_TAG: LazyLock<Regex> = LazyLock::new(|| {
    // Only package-qualified commands are strong enough evidence to call a
    // Claude command a skill invocation. Unqualified slash commands can be
    // ordinary built-ins or plugin commands and remain intentionally ignored.
    Regex::new(
        r"(?s)<command-message>\s*/?([A-Za-z0-9_.-]+:[A-Za-z0-9_.:-]+)\s*</command-message>|<command-name>\s*/?([A-Za-z0-9_.-]+:[A-Za-z0-9_.:-]+)\s*</command-name>",
    )
    .expect("static regex")
});

fn collect_claude_text(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => out.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    out.push(text.to_string());
                } else {
                    for field in ["text", "content"] {
                        if let Some(text) = item.get(field).and_then(serde_json::Value::as_str) {
                            out.push(text.to_string());
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn claude_record_is_user(value: &serde_json::Value) -> bool {
    value
        .pointer("/message/role")
        .or_else(|| value.get("role"))
        .or_else(|| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|role| matches!(role, "user" | "human"))
}

fn claude_event_identity(event: &ExtractedSkillEvent) -> String {
    if event.skill_name.contains(':') {
        event.skill_name.clone()
    } else if let Some(plugin) = event.skill_plugin.as_deref() {
        format!("{plugin}:{}", event.skill_name)
    } else {
        event.skill_name.clone()
    }
}

/// Extract Claude skill events from a raw transcript JSON value.
///
/// Supported evidence shapes:
/// - structured `attributionSkill` / `attributionPlugin` fields at top-level
///   or under `message.*`;
/// - observed package-qualified command envelopes such as
///   `<command-message>vibin:repo-status</command-message>` and
///   `<command-name>/vibin:repo-status</command-name>` in message content.
///
/// Duplicate evidence for the same skill on one transcript row is collapsed,
/// preferring the structured attribution when both forms are present.
pub fn extract_claude_skill_events(value: &serde_json::Value) -> Vec<ExtractedSkillEvent> {
    let mut events = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let candidates = [
        value,
        value.get("message").unwrap_or(&serde_json::Value::Null),
    ];
    for candidate in candidates {
        let Some(skill) = candidate
            .get("attributionSkill")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let plugin = candidate
            .get("attributionPlugin")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let event = ExtractedSkillEvent {
            skill_name: skill.to_string(),
            skill_plugin: plugin,
            event_kind: SkillEventKind::ClaudeAttribution,
            evidence_kind: SkillEvidenceKind::StructuredJsonField,
        };
        if let Some(normalized) = event.normalized() {
            seen.insert(claude_event_identity(&normalized));
            events.push(normalized);
        }
        break;
    }

    if !claude_record_is_user(value) {
        return events;
    }

    let mut text = Vec::new();
    if let Some(content) = value.get("content") {
        collect_claude_text(content, &mut text);
    }
    if let Some(content) = value.pointer("/message/content") {
        collect_claude_text(content, &mut text);
    }
    for fragment in text {
        for captures in CLAUDE_SKILL_COMMAND_TAG.captures_iter(&fragment) {
            let Some(raw_name) = captures.get(1).or_else(|| captures.get(2)) else {
                continue;
            };
            let event = ExtractedSkillEvent {
                skill_name: raw_name.as_str().to_string(),
                skill_plugin: None,
                event_kind: SkillEventKind::ClaudeSkillCommand,
                evidence_kind: SkillEvidenceKind::TranscriptContent,
            };
            let Some(normalized) = event.normalized() else {
                continue;
            };
            if seen.insert(claude_event_identity(&normalized)) {
                events.push(normalized);
            }
        }
    }
    events
}

/// Matches legacy name-only blocks or a complete native name/path header.
/// A truncated body is allowed; a name cannot cross an XML tag boundary.
static CODEX_SKILL_TAG: LazyLock<Regex> = LazyLock::new(|| {
    // Current Codex emits a path and the entire skill body after the name.
    // The forwarding byte budget can truncate that body, so require the
    // complete structured header, not the closing tag beyond the budget.
    // Keep the historical name-only form and reject ordinary catalog paths.
    Regex::new(r"(?s)<skill>\s*<name>\s*([^<]*?)\s*</name>\s*(?:</skill>|<path>[^<>\r\n]+/SKILL\.md</path>)")
        .expect("static regex")
});

/// Extract Codex skill-invocation events from transcript message text. Scans
/// for native skill headers (one row may contain several), de-duplicating
/// names within the row. Does not treat a command-read marker in plain text
/// as structured evidence; that requires separate parser provenance.
///
/// Eng review Fix 1: short-circuits on a cheap substring check before
/// touching the regex engine at all — the overwhelming majority of
/// transcript rows contain no skill tag, so this bounds the common case to
/// one `str::contains` call instead of a full regex scan.
#[cfg(test)]
pub fn extract_codex_skill_events(text: &str) -> Vec<ExtractedSkillEvent> {
    extract_codex_skill_events_with_kind(text, None)
}

/// The event kind is parser provenance, carried separately from message text
/// in forwarded envelopes and persisted metadata. Legacy marker-only records
/// cannot prove a read; recover them from the original native source instead.
pub fn extract_codex_skill_events_with_kind(
    text: &str,
    event_kind: Option<&str>,
) -> Vec<ExtractedSkillEvent> {
    if event_kind == Some("codex_skill_read") && text.starts_with("{\"cortex_skill_read\":") {
        let name = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| {
                value
                    .get("cortex_skill_read")
                    .and_then(|name| name.as_str())
                    .map(str::to_owned)
            });
        return name
            .and_then(|skill_name| {
                ExtractedSkillEvent {
                    skill_name,
                    skill_plugin: None,
                    event_kind: SkillEventKind::CodexSkillRead,
                    evidence_kind: SkillEvidenceKind::StructuredJsonField,
                }
                .normalized()
            })
            .into_iter()
            .collect();
    }
    if !text.contains("<skill>") {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut events = Vec::new();
    for capture in CODEX_SKILL_TAG.captures_iter(text) {
        let raw_name = capture.get(1).map_or("", |m| m.as_str());
        let event = ExtractedSkillEvent {
            skill_name: raw_name.to_string(),
            skill_plugin: None,
            event_kind: SkillEventKind::CodexSkillBlock,
            evidence_kind: SkillEvidenceKind::TranscriptContent,
        };
        let Some(normalized) = event.normalized() else {
            continue;
        };
        if seen.insert(normalized.skill_name.clone()) {
            events.push(normalized);
        }
    }
    events
}

/// Native Codex command-completion evidence, not a path guessed from shell
/// text. Restrict this to a single parsed read with a successful completion:
/// a multi-command shell's final exit code cannot prove each read succeeded.
pub(crate) fn codex_skill_read_summary(value: &serde_json::Value) -> Option<String> {
    if value.get("type")?.as_str()? != "event_msg"
        || value.pointer("/payload/type")?.as_str()? != "item_completed"
    {
        return None;
    }
    let item = value.pointer("/payload/item")?;
    if item.get("type")?.as_str()? != "CommandExecution"
        || item.get("status")?.as_str()? != "completed"
        || item.get("exit_code")?.as_i64()? != 0
        || item.get("aggregated_output")?.as_str()?.trim().is_empty()
    {
        return None;
    }
    let commands = item.get("parsed_cmd")?.as_array()?;
    if commands.len() != 1 || commands[0].get("type")?.as_str()? != "read" {
        return None;
    }
    let path = commands[0].get("path")?.as_str()?;
    let directory = path.strip_suffix("/SKILL.md")?;
    let name = directory.rsplit('/').next()?;
    if name.is_empty()
        || name.len() > MAX_SKILL_FIELD_CHARS
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte))
    {
        return None;
    }
    // These versioned installation layouts establish plugin identity without
    // consulting mutable files. Arbitrary project paths instead receive an
    // unresolved digest identity: no private path or guessed plugin escapes.
    let parts = path.split('/').collect::<Vec<_>>();
    let plugin = parts.len().checked_sub(9).and_then(|start| {
        let parts = &parts[start..];
        (matches!(parts[0], ".codex" | ".claude")
            && parts[1] == "plugins"
            && parts[2] == "cache"
            && parts[6] == "skills"
            && parts[8] == "SKILL.md"
            && parts[3..8]
                .iter()
                .all(|part| !part.is_empty() && *part != "." && *part != "..")
            && parts[4]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte)))
        .then_some(parts[4])
    });
    let user_skill = parts.len().checked_sub(4).is_some_and(|start| {
        let parts = &parts[start..];
        matches!(parts[0], ".codex" | ".claude")
            && parts[1] == "skills"
            && parts[2] == name
            && parts[3] == "SKILL.md"
    });
    let name = match plugin {
        Some(plugin) if plugin.len() + 1 + name.len() <= MAX_SKILL_FIELD_CHARS => {
            format!("{plugin}:{name}")
        }
        None if user_skill => name.to_owned(),
        _ => format!(
            "unresolved-skill-read-{:x}",
            Sha256::digest(path.as_bytes())
        ),
    };
    Some(serde_json::json!({"cortex_skill_read": name}).to_string())
}

#[cfg(test)]
#[path = "skill_events_tests.rs"]
mod tests;
