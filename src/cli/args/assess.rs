//! `cortex assess` — unified verb namespace for LLM-guarded and
//! deterministic incident assessment. `Mcp` and `Hooks` are both implemented
//! (GH #104/#105).

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssessCommand {
    Skill(AssessSkillArgs),
    Abuse(AssessAbuseArgs),
    Mcp(AssessMcpArgs),
    Hooks(AssessHooksArgs),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AssessHooksArgs {
    pub incident_id: Option<String>,
    /// Narrow to a known hook by name (`--hook NAME`).
    pub hook_name: Option<String>,
    /// Narrow to a hook event (e.g. `PostToolUse`) via `--hook-event`.
    pub hook_event: Option<String>,
    pub hook_source: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
    pub limit: Option<u32>,
    pub all: bool,
    pub no_llm: bool,
    pub json: bool,
    /// When true, collect a fresh point-in-time hook config inventory from the
    /// local host (`~/.claude/settings.json`, `~/.codex/hooks.json`,
    /// `~/.codex/config.toml [hooks.state]`) before assessing, so config/trust
    /// evidence is available alongside runtime evidence.
    pub collect_config: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AssessSkillArgs {
    pub incident_id: Option<String>,
    pub skill: Option<String>,
    pub plugin: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
    pub limit: Option<u32>,
    pub all: bool,
    pub no_llm: bool,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AssessAbuseArgs {
    pub incident_id: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
    pub limit: Option<u32>,
    pub no_llm: bool,
    pub json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AssessMcpArgs {
    pub incident_id: Option<String>,
    /// Bare positional argument — an mcp_server, mcp_tool, or raw tool
    /// name. `None` when the caller used `--server`/`--tool` flags only.
    pub target: Option<String>,
    pub server: Option<String>,
    pub tool_name: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub window_minutes: Option<u32>,
    pub correlation_window_minutes: Option<u32>,
    pub limit: Option<u32>,
    pub all: bool,
    pub no_llm: bool,
    pub json: bool,
}
