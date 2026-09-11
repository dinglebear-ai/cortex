//! `cortex assess` — unified verb namespace for LLM-guarded and
//! deterministic incident assessment. `mcp` (GH #104) and `hooks` (GH #105)
//! are both fully implemented in this file.

use anyhow::{Result, anyhow, bail};

use super::super::args::{
    AssessAbuseArgs, AssessCommand, AssessHooksArgs, AssessMcpArgs, AssessSkillArgs, CliCommand,
};
use super::super::parse_common::{FlagCursor, norm_time, parse_u32_flag};
use super::super::suggest;

pub(crate) fn parse_assess(args: &[String]) -> Result<CliCommand> {
    let (subcommand, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("assess requires a subcommand: skill, abuse, mcp, hooks"))?;
    match subcommand.as_str() {
        "skill" => parse_assess_skill_from(rest),
        "abuse" => parse_assess_abuse(rest),
        "mcp" => parse_assess_mcp_from(rest),
        "hooks" => parse_assess_hooks(rest),
        _ => bail!(
            "{}",
            suggest::unknown_command(
                "assess subcommand",
                subcommand,
                &["skill", "abuse", "mcp", "hooks"],
            )
        ),
    }
}

/// `pub(crate)` (not private) because `cortex sessions skillassess`
/// forwards to this directly so the two entry points never drift on flag
/// parsing.
pub(crate) fn parse_assess_skill_from(args: &[String]) -> Result<CliCommand> {
    let mut parsed = AssessSkillArgs::default();
    let mut positional: Option<String> = None;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--all" => parsed.all = true,
            "--no-llm" => parsed.no_llm = true,
            "--plugin" => parsed.plugin = Some(flags.value("--plugin")?),
            "--incident-id" => parsed.incident_id = Some(flags.value("--incident-id")?),
            "--model" => parsed.model = Some(flags.value("--model")?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "--window-minutes" => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    flags.value("--window-minutes")?,
                )?)
            }
            "--correlation-window-minutes" => {
                parsed.correlation_window_minutes = Some(parse_u32_flag(
                    "--correlation-window-minutes",
                    flags.value("--correlation-window-minutes")?,
                )?)
            }
            other if !other.starts_with('-') && positional.is_none() => {
                positional = Some(other.to_string());
            }
            other => bail!(
                "{}",
                suggest::unknown_option(
                    "assess skill",
                    other,
                    &[
                        "--json",
                        "--all",
                        "--no-llm",
                        "--plugin",
                        "--incident-id",
                        "--model",
                        "--project",
                        "--tool",
                        "--since",
                        "--until",
                        "--limit",
                        "--window-minutes",
                        "--correlation-window-minutes",
                    ],
                )
            ),
        }
    }
    parsed.skill = positional;
    if parsed.skill.is_none() && parsed.plugin.is_none() && parsed.incident_id.is_none() {
        bail!(
            "assess skill: skill name, --plugin, or --incident-id is required, e.g. `cortex assess skill frustration-assessment` or `cortex assess skill --plugin lavra`"
        );
    }
    Ok(CliCommand::Assess(AssessCommand::Skill(parsed)))
}

pub(crate) fn parse_assess_abuse(args: &[String]) -> Result<CliCommand> {
    let mut parsed = AssessAbuseArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--no-llm" => parsed.no_llm = true,
            "--incident-id" => parsed.incident_id = Some(flags.value("--incident-id")?),
            "--model" => parsed.model = Some(flags.value("--model")?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "--window-minutes" => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    flags.value("--window-minutes")?,
                )?)
            }
            "--correlation-window-minutes" => {
                parsed.correlation_window_minutes = Some(parse_u32_flag(
                    "--correlation-window-minutes",
                    flags.value("--correlation-window-minutes")?,
                )?)
            }
            other => bail!(
                "{}",
                suggest::unknown_option(
                    "assess abuse",
                    other,
                    &[
                        "--json",
                        "--no-llm",
                        "--incident-id",
                        "--model",
                        "--project",
                        "--tool",
                        "--since",
                        "--until",
                        "--limit",
                        "--window-minutes",
                        "--correlation-window-minutes",
                    ],
                )
            ),
        }
    }
    Ok(CliCommand::Assess(AssessCommand::Abuse(parsed)))
}

/// `pub(crate)` (not private) because `cortex sessions mcpassess` forwards
/// to this directly so the two entry points never drift on flag parsing,
/// mirroring `parse_assess_skill_from`.
pub(crate) fn parse_assess_mcp_from(args: &[String]) -> Result<CliCommand> {
    let mut parsed = AssessMcpArgs::default();
    let mut positional: Option<String> = None;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--all" => parsed.all = true,
            "--no-llm" => parsed.no_llm = true,
            "--server" => parsed.server = Some(flags.value("--server")?),
            "--tool-name" => parsed.tool_name = Some(flags.value("--tool-name")?),
            "--incident-id" => parsed.incident_id = Some(flags.value("--incident-id")?),
            "--model" => parsed.model = Some(flags.value("--model")?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "--window-minutes" => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    flags.value("--window-minutes")?,
                )?)
            }
            "--correlation-window-minutes" => {
                parsed.correlation_window_minutes = Some(parse_u32_flag(
                    "--correlation-window-minutes",
                    flags.value("--correlation-window-minutes")?,
                )?)
            }
            other if !other.starts_with('-') && positional.is_none() => {
                positional = Some(other.to_string());
            }
            other => bail!(
                "{}",
                suggest::unknown_option(
                    "assess mcp",
                    other,
                    &[
                        "--json",
                        "--all",
                        "--no-llm",
                        "--server",
                        "--tool-name",
                        "--incident-id",
                        "--model",
                        "--project",
                        "--tool",
                        "--since",
                        "--until",
                        "--limit",
                        "--window-minutes",
                        "--correlation-window-minutes",
                    ],
                )
            ),
        }
    }
    parsed.target = positional;
    if parsed.target.is_none()
        && parsed.server.is_none()
        && parsed.tool_name.is_none()
        && parsed.incident_id.is_none()
    {
        bail!(
            "assess mcp: an mcp server/tool name is required, e.g. `cortex assess mcp cortex` or `cortex assess mcp --server labby --tool-name search`"
        );
    }
    Ok(CliCommand::Assess(AssessCommand::Mcp(parsed)))
}

/// Parse `cortex assess hooks [--hook NAME] [--hook-event EVENT] [--since ...]
/// [--project ...] [--tool ...] [--all|--limit N] [--no-llm] [--collect-config]`.
/// A positional argument (if given) is treated as `--hook NAME`, mirroring the
/// skill parser's positional skill name.
pub(crate) fn parse_assess_hooks(args: &[String]) -> Result<CliCommand> {
    let mut parsed = AssessHooksArgs::default();
    let mut positional: Option<String> = None;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--all" => parsed.all = true,
            "--no-llm" => parsed.no_llm = true,
            "--collect-config" => parsed.collect_config = true,
            "--hook" => parsed.hook_name = Some(flags.value("--hook")?),
            "--incident-id" => parsed.incident_id = Some(flags.value("--incident-id")?),
            "--hook-event" => parsed.hook_event = Some(flags.value("--hook-event")?),
            "--hook-source" => parsed.hook_source = Some(flags.value("--hook-source")?),
            "--model" => parsed.model = Some(flags.value("--model")?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "--window-minutes" => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    flags.value("--window-minutes")?,
                )?)
            }
            "--correlation-window-minutes" => {
                parsed.correlation_window_minutes = Some(parse_u32_flag(
                    "--correlation-window-minutes",
                    flags.value("--correlation-window-minutes")?,
                )?)
            }
            other if !other.starts_with('-') && positional.is_none() => {
                positional = Some(other.to_string());
            }
            other => bail!(
                "{}",
                suggest::unknown_option(
                    "assess hooks",
                    other,
                    &[
                        "--json",
                        "--all",
                        "--no-llm",
                        "--collect-config",
                        "--hook",
                        "--incident-id",
                        "--hook-event",
                        "--hook-source",
                        "--model",
                        "--project",
                        "--tool",
                        "--since",
                        "--until",
                        "--limit",
                        "--window-minutes",
                        "--correlation-window-minutes",
                    ],
                )
            ),
        }
    }
    // A bare positional is the hook name (unless --hook already set it).
    if parsed.hook_name.is_none() {
        parsed.hook_name = positional;
    }
    Ok(CliCommand::Assess(AssessCommand::Hooks(parsed)))
}

#[cfg(test)]
#[path = "assess_tests.rs"]
mod tests;
