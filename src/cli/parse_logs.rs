use anyhow::{Result, bail};

use super::argdefaults::positional_value;
use super::parse_common::{FlagCursor, parse_output_args, parse_u32_flag, value_after_equals};
use super::{
    CliCommand, CorrelateArgs, FilterArgs, HostsCommand, IncidentArgs, IngestRateArgs,
    PatternsArgs, SearchArgs, SessionsArgs, SessionsCommand, SilentHostsArgs, SourceIpsArgs,
    TailArgs, TimeRangeArgs, TimelineArgs,
};
use cortex::app::parse_time_arg;

/// Normalize a user time value (relative or absolute) to RFC3339 at parse time.
fn norm_time(raw: String) -> Result<String> {
    parse_time_arg(&raw, chrono::Utc::now()).map_err(|e| anyhow::anyhow!("{e}"))
}

pub(crate) fn parse_search(args: &[String]) -> Result<CliCommand> {
    let mut parsed = SearchArgs::default();
    let mut query = Vec::new();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--host" => parsed.host = Some(flags.value("--host")?),
            "--source" => parsed.source = Some(flags.value("--source")?),
            "--severity" => parsed.severity = Some(flags.value("--severity")?),
            "--app" => parsed.app = Some(flags.value("--app")?),
            "--facility" => parsed.facility = Some(flags.value("--facility")?),
            "--exclude-facility" => {
                parsed.exclude_facility = Some(flags.value("--exclude-facility")?)
            }
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--received-since" => {
                parsed.received_since = Some(norm_time(flags.value("--received-since")?)?)
            }
            "--received-until" => {
                parsed.received_until = Some(norm_time(flags.value("--received-until")?)?)
            }
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "-h" | "--help" => bail!("use `cortex --help` for usage"),
            _ if arg.starts_with("--host=") => {
                parsed.host = Some(value_after_equals(arg, "--host")?)
            }
            _ if arg.starts_with("--source=") => {
                parsed.source = Some(value_after_equals(arg, "--source")?)
            }
            _ if arg.starts_with("--severity=") => {
                parsed.severity = Some(value_after_equals(arg, "--severity")?)
            }
            _ if arg.starts_with("--app=") => parsed.app = Some(value_after_equals(arg, "--app")?),
            _ if arg.starts_with("--facility=") => {
                parsed.facility = Some(value_after_equals(arg, "--facility")?)
            }
            _ if arg.starts_with("--exclude-facility=") => {
                parsed.exclude_facility = Some(value_after_equals(arg, "--exclude-facility")?)
            }
            _ if arg.starts_with("--since=") => {
                parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
            }
            _ if arg.starts_with("--until=") => {
                parsed.until = Some(norm_time(value_after_equals(arg, "--until")?)?)
            }
            _ if arg.starts_with("--received-since=") => {
                parsed.received_since =
                    Some(norm_time(value_after_equals(arg, "--received-since")?)?)
            }
            _ if arg.starts_with("--received-until=") => {
                parsed.received_until =
                    Some(norm_time(value_after_equals(arg, "--received-until")?)?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            "--grep" => parsed.grep = Some(flags.value("--grep")?),
            _ if arg.starts_with("--grep=") => {
                parsed.grep = Some(value_after_equals(arg, "--grep")?)
            }
            _ if arg.starts_with('-') => bail!("unknown search option: {arg}"),
            _ => query.push(arg),
        }
    }
    parsed.query = (!query.is_empty()).then(|| query.join(" "));
    if parsed.grep.is_some() && parsed.query.is_some() {
        bail!("--grep and a positional query are mutually exclusive; use one or the other");
    }
    if parsed.grep.as_deref().is_some_and(|g| g.trim().is_empty()) {
        bail!("--grep requires non-empty text");
    }
    Ok(CliCommand::Search(parsed))
}

pub(crate) fn parse_filter(args: &[String]) -> Result<CliCommand> {
    let mut parsed = FilterArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--host" => parsed.host = Some(flags.value("--host")?),
            "--source" => parsed.source = Some(flags.value("--source")?),
            "--severity" => parsed.severity = Some(flags.value("--severity")?),
            "--app" => parsed.app = Some(flags.value("--app")?),
            "--facility" => parsed.facility = Some(flags.value("--facility")?),
            "--exclude-facility" => {
                parsed.exclude_facility = Some(flags.value("--exclude-facility")?)
            }
            "--process-id" => parsed.process_id = Some(flags.value("--process-id")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--received-since" => {
                parsed.received_since = Some(norm_time(flags.value("--received-since")?)?)
            }
            "--received-until" => {
                parsed.received_until = Some(norm_time(flags.value("--received-until")?)?)
            }
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            "--source-kind" => parsed.source_kind = Some(flags.value("--source-kind")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--session-id" => parsed.session_id = Some(flags.value("--session-id")?),
            "--container" => parsed.container = Some(flags.value("--container")?),
            "--docker-host" => parsed.docker_host = Some(flags.value("--docker-host")?),
            "--stream" => parsed.stream = Some(flags.value("--stream")?),
            "--event-action" => parsed.event_action = Some(flags.value("--event-action")?),
            "-h" | "--help" => bail!("use `cortex --help` for usage"),
            _ if arg.starts_with("--host=") => {
                parsed.host = Some(value_after_equals(arg, "--host")?)
            }
            _ if arg.starts_with("--source=") => {
                parsed.source = Some(value_after_equals(arg, "--source")?)
            }
            _ if arg.starts_with("--severity=") => {
                parsed.severity = Some(value_after_equals(arg, "--severity")?)
            }
            _ if arg.starts_with("--app=") => parsed.app = Some(value_after_equals(arg, "--app")?),
            _ if arg.starts_with("--facility=") => {
                parsed.facility = Some(value_after_equals(arg, "--facility")?)
            }
            _ if arg.starts_with("--exclude-facility=") => {
                parsed.exclude_facility = Some(value_after_equals(arg, "--exclude-facility")?)
            }
            _ if arg.starts_with("--process-id=") => {
                parsed.process_id = Some(value_after_equals(arg, "--process-id")?)
            }
            _ if arg.starts_with("--since=") => {
                parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
            }
            _ if arg.starts_with("--until=") => {
                parsed.until = Some(norm_time(value_after_equals(arg, "--until")?)?)
            }
            _ if arg.starts_with("--received-since=") => {
                parsed.received_since =
                    Some(norm_time(value_after_equals(arg, "--received-since")?)?)
            }
            _ if arg.starts_with("--received-until=") => {
                parsed.received_until =
                    Some(norm_time(value_after_equals(arg, "--received-until")?)?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ if arg.starts_with("--source-kind=") => {
                parsed.source_kind = Some(value_after_equals(arg, "--source-kind")?)
            }
            _ if arg.starts_with("--tool=") => {
                parsed.tool = Some(value_after_equals(arg, "--tool")?)
            }
            _ if arg.starts_with("--project=") => {
                parsed.project = Some(value_after_equals(arg, "--project")?)
            }
            _ if arg.starts_with("--session-id=") => {
                parsed.session_id = Some(value_after_equals(arg, "--session-id")?)
            }
            _ if arg.starts_with("--container=") => {
                parsed.container = Some(value_after_equals(arg, "--container")?)
            }
            _ if arg.starts_with("--docker-host=") => {
                parsed.docker_host = Some(value_after_equals(arg, "--docker-host")?)
            }
            _ if arg.starts_with("--stream=") => {
                parsed.stream = Some(value_after_equals(arg, "--stream")?)
            }
            _ if arg.starts_with("--event-action=") => {
                parsed.event_action = Some(value_after_equals(arg, "--event-action")?)
            }
            _ if arg.starts_with('-') => bail!("unknown filter option: {arg}"),
            _ => {
                bail!("filter does not accept positional query terms; use `search` for FTS queries")
            }
        }
    }
    Ok(CliCommand::Filter(parsed))
}

pub(crate) fn parse_tail(args: &[String]) -> Result<CliCommand> {
    let mut parsed = TailArgs::default();
    let mut positionals: Vec<String> = Vec::new();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--host" => parsed.host = Some(flags.value("--host")?),
            "--source" => parsed.source = Some(flags.value("--source")?),
            "--app" => parsed.app = Some(flags.value("--app")?),
            "--n" | "-n" | "--limit" => parsed.n = Some(parse_u32_flag(&arg, flags.value(&arg)?)?),
            _ if arg.starts_with("--host=") => {
                parsed.host = Some(value_after_equals(arg, "--host")?)
            }
            _ if arg.starts_with("--source=") => {
                parsed.source = Some(value_after_equals(arg, "--source")?)
            }
            _ if arg.starts_with("--app=") => parsed.app = Some(value_after_equals(arg, "--app")?),
            _ if arg.starts_with("--n=") => {
                parsed.n = Some(parse_u32_flag("--n", value_after_equals(arg, "--n")?)?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.n = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ if arg.starts_with('-') => bail!("unknown tail option: {arg}"),
            // A bare positional binds to --host (e.g. `cortex tail devhost`); the
            // result count is set with -n/--limit.
            _ => positionals.push(arg),
        }
    }
    if let Some(host) = positional_value("tail", &positionals)? {
        if parsed.host.is_some() {
            bail!("--host and a positional host are mutually exclusive");
        }
        parsed.host = Some(host);
    }
    Ok(CliCommand::Tail(parsed))
}

pub(crate) fn parse_errors(args: &[String]) -> Result<CliCommand> {
    let mut parsed = TimeRangeArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            _ if arg.starts_with("--since=") => {
                parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
            }
            _ if arg.starts_with("--until=") => {
                parsed.until = Some(norm_time(value_after_equals(arg, "--until")?)?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ => bail!("unknown errors option: {arg}"),
        }
    }
    Ok(CliCommand::Errors(parsed))
}

pub(crate) fn parse_hosts(args: &[String]) -> Result<CliCommand> {
    let (subcommand, rest) = match args.split_first() {
        None => ("", args),
        Some((subcommand, _)) if subcommand.starts_with('-') => ("", args),
        Some((subcommand, rest)) => (subcommand.as_str(), rest),
    };
    match subcommand {
        "" => Ok(CliCommand::Hosts(HostsCommand::List(parse_output_args(
            "hosts", rest,
        )?))),
        "sources" => parse_hosts_sources(rest),
        "silent" => parse_hosts_silent(rest),
        _ => bail!(
            "{}",
            super::suggest::unknown_command("hosts subcommand", subcommand, &["sources", "silent"],)
        ),
    }
}

pub(crate) fn parse_sessions(args: &[String]) -> Result<CliCommand> {
    let mut parsed = SessionsArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--project" => parsed.project = Some(flags.value("--project")?),
            "--tool" => parsed.tool = Some(flags.value("--tool")?),
            "--host" => parsed.host = Some(flags.value("--host")?),
            "--since" => parsed.since = Some(norm_time(flags.value("--since")?)?),
            "--until" => parsed.until = Some(norm_time(flags.value("--until")?)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            _ if arg.starts_with("--project=") => {
                parsed.project = Some(value_after_equals(arg, "--project")?)
            }
            _ if arg.starts_with("--tool=") => {
                parsed.tool = Some(value_after_equals(arg, "--tool")?)
            }
            _ if arg.starts_with("--host=") => {
                parsed.host = Some(value_after_equals(arg, "--host")?)
            }
            _ if arg.starts_with("--since=") => {
                parsed.since = Some(norm_time(value_after_equals(arg, "--since")?)?)
            }
            _ if arg.starts_with("--until=") => {
                parsed.until = Some(norm_time(value_after_equals(arg, "--until")?)?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ if arg.starts_with('-') => bail!("unknown sessions option: {arg}"),
            _ => bail!("unexpected sessions argument: {arg}"),
        }
    }
    Ok(CliCommand::Sessions(SessionsCommand::List(parsed)))
}

pub(crate) fn parse_incident(args: &[String]) -> Result<CliCommand> {
    let mut parsed = IncidentArgs {
        minutes: Some(5),
        limit: Some(500),
        ..Default::default()
    };
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--around" => parsed.around = norm_time(flags.value("--around")?)?,
            "--minutes" => {
                parsed.minutes = Some(parse_u32_flag("--minutes", flags.value("--minutes")?)?)
            }
            "--service" => parsed.service = Some(flags.value("--service")?),
            "--host" => parsed.host = Some(flags.value(&arg)?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            _ if arg.starts_with("--around=") => {
                parsed.around = norm_time(value_after_equals(arg, "--around")?)?
            }
            _ if arg.starts_with("--minutes=") => {
                parsed.minutes = Some(parse_u32_flag(
                    "--minutes",
                    value_after_equals(arg, "--minutes")?,
                )?)
            }
            _ if arg.starts_with("--service=") => {
                parsed.service = Some(value_after_equals(arg, "--service")?)
            }
            _ if arg.starts_with("--host=") => {
                parsed.host = Some(value_after_equals(arg, "--host")?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ if arg.starts_with('-') => bail!("unknown incident option: {arg}"),
            _ if parsed.around.is_empty() => parsed.around = norm_time(arg)?,
            _ => bail!("unexpected incident argument: {arg}"),
        }
    }
    Ok(CliCommand::Incident(parsed))
}

pub(crate) fn parse_correlate(args: &[String]) -> Result<CliCommand> {
    let mut parsed = CorrelateArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--reference-time" => {
                parsed.reference_time = Some(norm_time(flags.value("--reference-time")?)?)
            }
            "--window-minutes" => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    flags.value("--window-minutes")?,
                )?)
            }
            "--severity-min" => parsed.severity_min = Some(flags.value("--severity-min")?),
            "--host" => parsed.host = Some(flags.value("--host")?),
            "--source" => parsed.source = Some(flags.value("--source")?),
            "--query" => parsed.query = Some(flags.value("--query")?),
            "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
            _ if arg.starts_with("--reference-time=") => {
                parsed.reference_time =
                    Some(norm_time(value_after_equals(arg, "--reference-time")?)?)
            }
            _ if arg.starts_with("--window-minutes=") => {
                parsed.window_minutes = Some(parse_u32_flag(
                    "--window-minutes",
                    value_after_equals(arg, "--window-minutes")?,
                )?)
            }
            _ if arg.starts_with("--severity-min=") => {
                parsed.severity_min = Some(value_after_equals(arg, "--severity-min")?)
            }
            _ if arg.starts_with("--host=") => {
                parsed.host = Some(value_after_equals(arg, "--host")?)
            }
            _ if arg.starts_with("--source=") => {
                parsed.source = Some(value_after_equals(arg, "--source")?)
            }
            _ if arg.starts_with("--query=") => {
                parsed.query = Some(value_after_equals(arg, "--query")?)
            }
            _ if arg.starts_with("--limit=") => {
                parsed.limit = Some(parse_u32_flag(
                    "--limit",
                    value_after_equals(arg, "--limit")?,
                )?)
            }
            _ if arg.starts_with('-') => bail!("unknown correlate option: {arg}"),
            _ if parsed.reference_time.is_none() => {
                // correlate's sole positional is the reference *time*. A non-time
                // value (a hostname, app, …) otherwise hits norm_time and yields
                // a cryptic "unrecognized time value" — redirect to the command
                // that actually correlates by entity.
                parsed.reference_time = Some(norm_time(arg.clone()).map_err(|_| {
                    anyhow::anyhow!(
                        "correlate events' positional argument is a reference time (e.g. `1h`, `2026-06-01`, or an RFC3339 timestamp), but got `{arg}`. \
To correlate everything related to a host, app, or topic, use `cortex correlate topic {arg}`; \
to anchor correlate on a time and filter by host, pass `--reference-time <time> --host {arg}`, \
or omit --reference-time and pass --query \"<text>\" to derive it from a matching AI session."
                    )
                })?);
            }
            _ => bail!("unexpected correlate argument: {arg}"),
        }
    }
    Ok(CliCommand::Correlate(parsed))
}

pub(crate) fn parse_hosts_sources(args: &[String]) -> Result<CliCommand> {
    let mut parsed = SourceIpsArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--limit")? {
            parsed.limit = Some(parse_u32_flag("--limit", v)?);
        } else if let Some(v) = flags.match_value(&arg, "--offset")? {
            parsed.offset = Some(parse_u32_flag("--offset", v)?);
        } else {
            bail!("unknown hosts sources option: {arg}");
        }
    }
    Ok(CliCommand::Hosts(HostsCommand::Sources(parsed)))
}

pub(crate) fn parse_hosts_silent(args: &[String]) -> Result<CliCommand> {
    let mut parsed = SilentHostsArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--silent-minutes")? {
            parsed.silent_minutes = Some(parse_u32_flag("--silent-minutes", v)?);
        } else {
            bail!("unknown hosts silent option: {arg}");
        }
    }
    Ok(CliCommand::Hosts(HostsCommand::Silent(parsed)))
}

pub(crate) fn parse_timeline(args: &[String]) -> Result<CliCommand> {
    let mut parsed = TimelineArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--bucket")? {
            parsed.bucket = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--group-by")? {
            parsed.group_by = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--since")? {
            parsed.since = Some(norm_time(v)?);
        } else if let Some(v) = flags.match_value(&arg, "--until")? {
            parsed.until = Some(norm_time(v)?);
        } else if let Some(v) = flags.match_value(&arg, "--host")? {
            parsed.host = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--app")? {
            parsed.app = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--severity-min")? {
            parsed.severity_min = Some(v);
        } else {
            bail!("unknown timeline option: {arg}");
        }
    }
    Ok(CliCommand::Timeline(parsed))
}

pub(crate) fn parse_patterns(args: &[String]) -> Result<CliCommand> {
    let mut parsed = PatternsArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(v) = flags.match_value(&arg, "--since")? {
            parsed.since = Some(norm_time(v)?);
        } else if let Some(v) = flags.match_value(&arg, "--until")? {
            parsed.until = Some(norm_time(v)?);
        } else if let Some(v) = flags.match_value(&arg, "--host")? {
            parsed.host = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--app")? {
            parsed.app = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--severity-min")? {
            parsed.severity_min = Some(v);
        } else if let Some(v) = flags.match_value(&arg, "--scan-limit")? {
            parsed.scan_limit = Some(parse_u32_flag("--scan-limit", v)?);
        } else if let Some(v) = flags.match_value(&arg, "--top-n")? {
            parsed.top_n = Some(parse_u32_flag("--top-n", v)?);
        } else if let Some(v) = flags.match_value(&arg, "--limit")? {
            parsed.top_n = Some(parse_u32_flag("--limit", v)?);
        } else {
            bail!("unknown patterns option: {arg}");
        }
    }
    Ok(CliCommand::Patterns(parsed))
}

pub(crate) fn parse_ingest_rate_args(args: &[String]) -> Result<IngestRateArgs> {
    let mut parsed = IngestRateArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--by-host" => parsed.by_host = true,
            _ => bail!("unknown ingestrate option: {arg}"),
        }
    }
    Ok(parsed)
}

#[cfg(test)]
#[path = "parse_logs_tests.rs"]
mod tests;
