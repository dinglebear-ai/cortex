//! Parser for `cortex reflect <skills|mcp|hooks>`.

use std::path::PathBuf;

use anyhow::{Result, bail};
use cortex::app::ReflectKind;

use super::super::args::{CliCommand, ReflectArgs};
use super::super::parse_common::{FlagCursor, norm_time, parse_u32_flag};
use super::super::suggest;

pub(crate) const DEFAULT_REFLECT_SINCE: &str = "7d";
pub(crate) const DEFAULT_REFLECT_MAX_ASSESS: u32 = 5;

const REFLECT_USAGE: &str = "reflect: which kind? use `cortex reflect skills`, `cortex reflect mcp`, or `cortex reflect hooks`";

const REFLECT_FLAGS: &[&str] = &[
    "--since",
    "--until",
    "--project",
    "--tool",
    "--no-llm",
    "--max-assess",
    "--no-index",
    "--db",
    "--json",
];

pub(crate) fn parse_reflect(args: &[String]) -> Result<CliCommand> {
    let mut since: Option<String> = None;
    let mut kind: Option<ReflectKind> = None;
    let mut parsed = ReflectArgs {
        since: String::new(),
        until: None,
        project: None,
        tool: None,
        kinds: Vec::new(),
        no_llm: false,
        max_assess: DEFAULT_REFLECT_MAX_ASSESS,
        no_index: false,
        db: None,
        json: false,
    };
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--no-llm" => parsed.no_llm = true,
            "--no-index" => parsed.no_index = true,
            "--since" => since = Some(flags.value("--since")?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--max-assess" => {
                parsed.max_assess = parse_u32_flag("--max-assess", flags.value("--max-assess")?)?
            }
            "--db" => parsed.db = Some(PathBuf::from(flags.value("--db")?)),
            other if !other.starts_with('-') && kind.is_none() => {
                kind = Some(parse_kind(other)?);
            }
            other if !other.starts_with('-') => {
                bail!("reflect takes one kind; got a second one: '{other}'")
            }
            other => bail!(
                "{}",
                suggest::unknown_option("reflect", other, REFLECT_FLAGS)
            ),
        }
    }
    let Some(kind) = kind else {
        bail!("{REFLECT_USAGE}");
    };
    parsed.kinds = vec![kind];
    parsed.since = norm_time(since.unwrap_or_else(|| DEFAULT_REFLECT_SINCE.to_string()))?;
    Ok(CliCommand::Reflect(parsed))
}

fn parse_kind(raw: &str) -> Result<ReflectKind> {
    match raw {
        "skills" | "skill" => Ok(ReflectKind::Skill),
        "mcp" => Ok(ReflectKind::Mcp),
        "hooks" | "hook" => Ok(ReflectKind::Hook),
        other => bail!("reflect: unknown kind '{other}'; expected skills, mcp, or hooks"),
    }
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
