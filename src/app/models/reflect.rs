//! Types for `cortex reflect`: one merged, ranked view over skill, MCP, and
//! hook incidents, plus the report the CLI renders.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReflectKind {
    Skill,
    Mcp,
    Hook,
}

impl ReflectKind {
    pub const ALL: [ReflectKind; 3] = [ReflectKind::Skill, ReflectKind::Mcp, ReflectKind::Hook];

    pub fn as_str(self) -> &'static str {
        match self {
            ReflectKind::Skill => "skill",
            ReflectKind::Mcp => "mcp",
            ReflectKind::Hook => "hook",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "skill" => Some(ReflectKind::Skill),
            "mcp" => Some(ReflectKind::Mcp),
            "hook" => Some(ReflectKind::Hook),
            _ => None,
        }
    }

    /// The `cortex assess <subcommand>` that assesses this kind.
    pub fn assess_subcommand(self) -> &'static str {
        match self {
            ReflectKind::Skill => "skill",
            ReflectKind::Mcp => "mcp",
            ReflectKind::Hook => "hooks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectRequest {
    pub since: Option<String>,
    pub until: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub kinds: Vec<ReflectKind>,
    pub run_llm: bool,
    pub max_assess: u32,
    pub index: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectIncident {
    pub kind: ReflectKind,
    pub incident_id: String,
    /// Display form: `plugin:skill`, `server/tool`, or `event:hook`.
    pub target: String,
    /// Exact grouping key: skill name, MCP server, or hook event.
    pub target_key: String,
    /// Optional second key: skill plugin, MCP tool, or hook name.
    pub target_detail: Option<String>,
    pub tool: String,
    pub project: String,
    pub session_id: String,
    pub last_seen: String,
    pub priority_score: f64,
    pub priority_label: String,
    pub signals_present: Vec<String>,
}

impl ReflectIncident {
    /// The command that assesses this incident on its own.
    pub fn assess_command(&self, since: Option<&str>, until: Option<&str>) -> String {
        let mut command = format!(
            "cortex assess {} --incident-id {}",
            self.kind.assess_subcommand(),
            self.incident_id
        );
        for (flag, value) in [("--since", since), ("--until", until)] {
            if let Some(value) = value {
                command.push(' ');
                command.push_str(flag);
                command.push(' ');
                command.push_str(value);
            }
        }
        command
    }
}

fn joined(first: &str, separator: char, second: Option<&str>) -> String {
    match second {
        Some(second) => format!("{first}{separator}{second}"),
        None => first.to_string(),
    }
}

impl From<SkillIncident> for ReflectIncident {
    fn from(incident: SkillIncident) -> Self {
        // Display puts the plugin first: `plugin:skill`.
        let target = match &incident.skill_plugin {
            Some(plugin) => format!("{plugin}:{}", incident.skill_name),
            None => incident.skill_name.clone(),
        };
        Self {
            kind: ReflectKind::Skill,
            incident_id: incident.incident_id,
            target,
            target_key: incident.skill_name,
            target_detail: incident.skill_plugin,
            tool: incident.tool,
            project: incident.project,
            session_id: incident.session_id,
            last_seen: incident.last_seen,
            priority_score: incident.priority_score,
            priority_label: incident.priority_label,
            signals_present: incident.signals_present,
        }
    }
}

impl From<McpIncident> for ReflectIncident {
    fn from(incident: McpIncident) -> Self {
        let target = joined(&incident.mcp_server, '/', incident.mcp_tool.as_deref());
        Self {
            kind: ReflectKind::Mcp,
            incident_id: incident.incident_id,
            target,
            target_key: incident.mcp_server,
            target_detail: incident.mcp_tool,
            tool: incident.tool,
            project: incident.project,
            session_id: incident.session_id,
            last_seen: incident.last_seen,
            priority_score: incident.priority_score,
            priority_label: incident.priority_label,
            signals_present: incident.signals_present,
        }
    }
}

impl From<HookIncident> for ReflectIncident {
    fn from(incident: HookIncident) -> Self {
        let target = joined(&incident.hook_event, ':', incident.hook_name.as_deref());
        Self {
            kind: ReflectKind::Hook,
            incident_id: incident.incident_id,
            target,
            target_key: incident.hook_event,
            target_detail: incident.hook_name,
            tool: incident.tool,
            project: incident.project,
            session_id: incident.session_id,
            last_seen: incident.last_seen,
            priority_score: incident.priority_score,
            priority_label: incident.priority_label,
            signals_present: incident.signals_present,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectKindListing {
    pub kind: ReflectKind,
    /// `total_incidents` from the list service (before its 100-row clamp).
    pub total: usize,
    /// True when the list or its candidate window was capped.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectKindSummary {
    pub kind: ReflectKind,
    pub listed: usize,
    pub total: usize,
    pub truncated: bool,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectMode {
    ReportOnly,
    ReportAndLlm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectIndexSummary {
    pub discovered_files: usize,
    pub ingested: usize,
    pub skipped_dupes: usize,
    pub parse_errors: usize,
}

impl From<&crate::scanner::IndexResult> for ReflectIndexSummary {
    fn from(result: &crate::scanner::IndexResult) -> Self {
        Self {
            discovered_files: result.discovered_files,
            ingested: result.ingested,
            skipped_dupes: result.skipped_dupes,
            parse_errors: result.parse_errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectAssessed {
    pub incident: ReflectIncident,
    /// LLM assessment Markdown. `None` in report-only mode or on failure.
    pub assessment: Option<String>,
    /// The kind's deterministic findings; `null` when the incident no
    /// longer resolved.
    pub findings: serde_json::Value,
    /// Why the LLM assessment or findings are missing.
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReflectReport {
    pub since: Option<String>,
    pub until: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub kinds: Vec<ReflectKind>,
    pub db_path: String,
    /// Where incidents came from: `local`, or the Cortex server URL.
    pub source: String,
    pub mode: ReflectMode,
    pub llm_fallback_reason: Option<String>,
    /// `None` when indexing was skipped with `--no-index`.
    pub index: Option<ReflectIndexSummary>,
    pub summary: Vec<ReflectKindSummary>,
    pub assessed: Vec<ReflectAssessed>,
    pub unassessed: Vec<ReflectIncident>,
}

/// Highest score first, then most recent, then incident id for a stable order.
pub fn rank_reflect_incidents(mut incidents: Vec<ReflectIncident>) -> Vec<ReflectIncident> {
    incidents.sort_by(|a, b| {
        b.priority_score
            .total_cmp(&a.priority_score)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.incident_id.cmp(&b.incident_id))
    });
    incidents
}

/// One row per listing, in listing order, including empty kinds.
pub fn summarize_reflect_incidents(
    listings: &[ReflectKindListing],
    incidents: &[ReflectIncident],
) -> Vec<ReflectKindSummary> {
    listings
        .iter()
        .map(|listing| {
            let mut summary = ReflectKindSummary {
                kind: listing.kind,
                listed: 0,
                total: 0,
                truncated: listing.truncated,
                critical: 0,
                high: 0,
                medium: 0,
                low: 0,
            };
            for incident in incidents
                .iter()
                .filter(|incident| incident.kind == listing.kind)
            {
                summary.listed += 1;
                match incident.priority_label.as_str() {
                    "critical" => summary.critical += 1,
                    "high" => summary.high += 1,
                    "medium" => summary.medium += 1,
                    _ => summary.low += 1,
                }
            }
            summary.total = listing.total.max(summary.listed);
            summary
        })
        .collect()
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
