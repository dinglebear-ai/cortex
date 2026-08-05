//! Parse function for `cortex topic-correlate`.
//!
//! Exposes the `topic_correlate` MCP action and `POST /api/topic-correlate`
//! REST route as a top-level CLI subcommand.

use anyhow::{Result, bail};

use super::super::args::{CliCommand, TopicCorrelateArgs};
use super::super::{FlagCursor, norm_time, parse_u32_flag};

pub(crate) fn parse_topic_correlate(args: &[String]) -> Result<CliCommand> {
    let mut parsed = TopicCorrelateArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--since")? {
            parsed.since = Some(norm_time(v)?);
        } else if let Some(v) = flags.match_value(&arg, "--until")? {
            parsed.until = Some(norm_time(v)?);
        } else if let Some(v) = flags.match_value(&arg, "--depth")? {
            parsed.depth = Some(parse_u32_flag("--depth", v)?.min(u32::from(u8::MAX)) as u8);
        } else if let Some(v) = flags.match_value(&arg, "--source-kinds")? {
            parsed.source_kinds = Some(
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
        } else if let Some(v) = flags.match_value(&arg, "--limit")? {
            parsed.limit = Some(parse_u32_flag("--limit", v)?);
        } else if arg.starts_with('-') {
            bail!("unknown topic-correlate option: {arg}");
        } else if parsed.topic.is_none() {
            parsed.topic = Some(arg);
        } else {
            // Additional bare terms extend the topic (e.g. `devhost dns adguard`).
            let topic = parsed.topic.get_or_insert_with(String::new);
            topic.push(' ');
            topic.push_str(&arg);
        }
    }
    if parsed.topic.as_deref().unwrap_or("").trim().is_empty() {
        bail!("topic-correlate requires a topic, e.g. `cortex topic-correlate axon`");
    }
    Ok(CliCommand::TopicCorrelate(parsed))
}
