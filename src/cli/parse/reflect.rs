//! Parser for `cortex reflect`.

use std::path::PathBuf;

use anyhow::{Result, bail};
use cortex::app::ReflectKind;

use super::super::args::{CliCommand, ReflectArgs};
use super::super::parse_common::{FlagCursor, norm_time, parse_u32_flag};
use super::super::suggest;

pub(crate) const DEFAULT_REFLECT_SINCE: &str = "7d";
pub(crate) const DEFAULT_REFLECT_MAX_ASSESS: u32 = 5;

const REFLECT_FLAGS: &[&str] = &[
    "--since",
    "--until",
    "--project",
    "--tool",
    "--kinds",
    "--no-llm",
    "--max-assess",
    "--no-index",
    "--db",
    "--json",
];

pub(crate) fn parse_reflect(args: &[String]) -> Result<CliCommand> {
    let mut since: Option<String> = None;
    let mut parsed = ReflectArgs {
        since: String::new(),
        until: None,
        project: None,
        tool: None,
        kinds: ReflectKind::ALL.to_vec(),
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
            "--kinds" => parsed.kinds = parse_kinds(&flags.value("--kinds")?)?,
            "--max-assess" => {
                parsed.max_assess = parse_u32_flag("--max-assess", flags.value("--max-assess")?)?
            }
            "--db" => parsed.db = Some(PathBuf::from(flags.value("--db")?)),
            other => bail!(
                "{}",
                suggest::unknown_option("reflect", other, REFLECT_FLAGS)
            ),
        }
    }
    parsed.since = norm_time(since.unwrap_or_else(|| DEFAULT_REFLECT_SINCE.to_string()))?;
    Ok(CliCommand::Reflect(parsed))
}

fn parse_kinds(raw: &str) -> Result<Vec<ReflectKind>> {
    let mut kinds = Vec::new();
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let Some(kind) = ReflectKind::parse(part) else {
            bail!("reflect: unknown kind '{part}'; expected skill, mcp, or hook");
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        bail!("reflect: --kinds needs at least one of skill, mcp, hook");
    }
    Ok(kinds)
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
