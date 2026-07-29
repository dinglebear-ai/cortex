use anyhow::{Result, anyhow, bail};
use cortex::compose::ComposeTarget;

use super::args::StatsCommand;
use super::parse_common::{FlagCursor, parse_output_args, parse_u32_flag, value_after_equals};
use super::{
    CliCommand, ComposeArgs, ComposeCommand, ComposeLogsArgs, ComposeMutationArgs, DbBackupArgs,
    DbCheckpointArgs, DbCommand, DbIntegrityArgs, DbIntegrityStatusArgs, DbStatusArgs,
    DbVacuumArgs, PluginHookArgs, ServiceLogsArgs, SetupArgs, SetupCommand,
};
pub(crate) fn parse_stats(args: &[String]) -> Result<CliCommand> {
    if let Some((subcommand, rest)) = args.split_first()
        && subcommand == "ingestrate"
    {
        return Ok(CliCommand::Stats(StatsCommand::IngestRate(
            super::parse_logs::parse_ingest_rate_args(rest)?,
        )));
    }
    Ok(CliCommand::Stats(StatsCommand::Summary(parse_output_args(
        "stats", args,
    )?)))
}

pub(crate) fn parse_status(args: &[String]) -> Result<CliCommand> {
    Ok(CliCommand::Status(parse_output_args("status", args)?))
}

fn parse_service_logs_args(context: &str, args: &[String]) -> Result<ServiceLogsArgs> {
    let (service, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("{context} requires a service name"))?;
    if service.starts_with('-') {
        bail!("{context} requires a service name");
    }
    let mut parsed = ServiceLogsArgs {
        service: service.clone(),
        ..Default::default()
    };
    let mut flags = FlagCursor::new(rest);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--since" => parsed.since = Some(flags.value("--since")?),
            "--until" => parsed.until = Some(flags.value("--until")?),
            "--tail" | "-n" => parsed.tail = Some(parse_u32_flag(&arg, flags.value(&arg)?)?),
            _ if arg.starts_with("--since=") => {
                parsed.since = Some(value_after_equals(arg, "--since")?)
            }
            _ if arg.starts_with("--until=") => {
                parsed.until = Some(value_after_equals(arg, "--until")?)
            }
            _ if arg.starts_with("--tail=") => {
                parsed.tail = Some(parse_u32_flag(
                    "--tail",
                    value_after_equals(arg, "--tail")?,
                )?)
            }
            _ if arg.starts_with("-n=") => {
                parsed.tail = Some(parse_u32_flag("-n", value_after_equals(arg, "-n")?)?)
            }
            _ if arg.starts_with('-') => bail!("unknown {context} option: {arg}"),
            _ => bail!("unexpected {context} argument: {arg}"),
        }
    }
    Ok(parsed)
}

pub(crate) fn parse_db(args: &[String]) -> Result<CliCommand> {
    let (subcommand, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("db requires a subcommand"))?;
    match subcommand.as_str() {
        "status" => parse_db_status(rest),
        "integrity" => parse_db_integrity(rest),
        "checkpoint" => parse_db_checkpoint(rest),
        "vacuum" => parse_db_vacuum(rest),
        "backup" => parse_db_backup(rest),
        _ => bail!(
            "{}",
            super::suggest::unknown_command(
                "db subcommand",
                subcommand,
                &["status", "integrity", "checkpoint", "vacuum", "backup"],
            )
        ),
    }
}

pub(crate) fn parse_db_status(args: &[String]) -> Result<CliCommand> {
    let mut parsed = DbStatusArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--check-coord" => parsed.check_coord = true,
            _ => bail!("unknown db status option: {arg}"),
        }
    }
    Ok(CliCommand::Db(DbCommand::Status(parsed)))
}

pub(crate) fn parse_db_integrity(args: &[String]) -> Result<CliCommand> {
    // `db integrity status <id>` polls a background job; everything else runs
    // (or starts) a check.
    if let Some((first, rest)) = args.split_first()
        && first == "status"
    {
        return parse_db_integrity_status(rest);
    }
    let mut parsed = DbIntegrityArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--quick" => parsed.quick = true,
            "--background" => parsed.background = true,
            _ => bail!("unknown db integrity option: {arg}"),
        }
    }
    Ok(CliCommand::Db(DbCommand::Integrity(parsed)))
}

pub(crate) fn parse_db_integrity_status(args: &[String]) -> Result<CliCommand> {
    let mut parsed = DbIntegrityStatusArgs::default();
    let mut job_id: Option<i64> = None;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            other if !other.starts_with("--") => {
                job_id = Some(
                    other
                        .parse::<i64>()
                        .map_err(|_| anyhow!("db integrity status: invalid job id '{other}'"))?,
                );
            }
            _ => bail!("unknown db integrity status option: {arg}"),
        }
    }
    parsed.job_id = job_id.ok_or_else(|| anyhow!("db integrity status requires a job id"))?;
    Ok(CliCommand::Db(DbCommand::IntegrityStatus(parsed)))
}

pub(crate) fn parse_db_checkpoint(args: &[String]) -> Result<CliCommand> {
    let mut parsed = DbCheckpointArgs::default();
    let mut mode = None;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--mode" => set_checkpoint_mode(&mut mode, flags.value("--mode")?)?,
            _ if arg.starts_with("--mode=") => {
                set_checkpoint_mode(&mut mode, value_after_equals(arg, "--mode")?)?
            }
            other if !other.starts_with('-') => set_checkpoint_mode(&mut mode, other.to_string())?,
            _ => bail!("unknown db checkpoint option: {arg}"),
        }
    }
    if let Some(mode) = mode {
        parsed.mode = mode;
    }
    match parsed.mode.as_str() {
        "passive" | "full" | "restart" | "truncate" => {}
        _ => bail!("--mode must be one of passive, full, restart, truncate"),
    }
    Ok(CliCommand::Db(DbCommand::Checkpoint(parsed)))
}

fn set_checkpoint_mode(mode: &mut Option<String>, value: String) -> Result<()> {
    if mode.replace(value).is_some() {
        bail!("db checkpoint mode may only be specified once");
    }
    Ok(())
}

pub(crate) fn parse_db_vacuum(args: &[String]) -> Result<CliCommand> {
    let mut parsed = DbVacuumArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--full" => parsed.full = true,
            "--force" => parsed.force = true,
            "--pages" => parsed.pages = parse_u32_flag("--pages", flags.value("--pages")?)?,
            _ if arg.starts_with("--pages=") => {
                parsed.pages = parse_u32_flag("--pages", value_after_equals(arg, "--pages")?)?
            }
            _ => bail!("unknown db vacuum option: {arg}"),
        }
    }
    if parsed.pages == 0 {
        bail!("--pages must be greater than zero");
    }
    Ok(CliCommand::Db(DbCommand::Vacuum(parsed)))
}

pub(crate) fn parse_db_backup(args: &[String]) -> Result<CliCommand> {
    let mut parsed = DbBackupArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--output" => parsed.output = Some(flags.value("--output")?),
            _ if arg.starts_with("--output=") => {
                parsed.output = Some(value_after_equals(arg, "--output")?)
            }
            other if !other.starts_with('-') && parsed.output.is_none() => {
                parsed.output = Some(other.to_string())
            }
            _ => bail!("unknown db backup option: {arg}"),
        }
    }
    Ok(CliCommand::Db(DbCommand::Backup(parsed)))
}

pub(crate) fn parse_compose(args: &[String]) -> Result<CliCommand> {
    let (subcommand, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("compose requires a subcommand"))?;
    match subcommand.as_str() {
        "status" => Ok(CliCommand::Compose(ComposeCommand::Status(
            parse_compose_args(rest)?,
        ))),
        "doctor" => Ok(CliCommand::Compose(ComposeCommand::Doctor(
            parse_compose_args(rest)?,
        ))),
        "up" => Ok(CliCommand::Compose(ComposeCommand::Up(
            parse_compose_mutation(rest, false)?,
        ))),
        "down" => Ok(CliCommand::Compose(ComposeCommand::Down(
            parse_compose_mutation(rest, true)?,
        ))),
        "restart" => Ok(CliCommand::Compose(ComposeCommand::Restart(
            parse_compose_mutation(rest, false)?,
        ))),
        "pull" => Ok(CliCommand::Compose(ComposeCommand::Pull(
            parse_compose_mutation(rest, false)?,
        ))),
        "logs" => parse_compose_logs_command(rest),
        "config" => bail!("cortex compose config is deferred from the first pass"),
        "upgrade" => bail!(
            "cortex compose upgrade is deferred; run `cortex compose pull` then `cortex compose up`"
        ),
        other => bail!(
            "{}",
            super::suggest::unknown_command(
                "compose subcommand",
                other,
                &[
                    "status", "doctor", "up", "down", "restart", "pull", "logs", "config",
                    "upgrade",
                ],
            )
        ),
    }
}

pub(crate) fn parse_setup(args: &[String]) -> Result<CliCommand> {
    let (subcommand, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("setup requires a subcommand"))?;
    match subcommand.as_str() {
        "check" => Ok(CliCommand::Setup(SetupCommand::Check(parse_setup_args(
            rest,
        )?))),
        "repair" => Ok(CliCommand::Setup(SetupCommand::Repair(parse_setup_args(
            rest,
        )?))),
        "install" => Ok(CliCommand::Setup(SetupCommand::Install(parse_setup_args(
            rest,
        )?))),
        "pluginhook" => Ok(CliCommand::Setup(SetupCommand::PluginHook(
            parse_plugin_hook_args(rest)?,
        ))),
        other => bail!(
            "{}",
            super::suggest::unknown_command(
                "setup subcommand",
                other,
                &["check", "repair", "install", "pluginhook"],
            )
        ),
    }
}

pub(crate) fn parse_setup_args(args: &[String]) -> Result<SetupArgs> {
    let mut parsed = SetupArgs::default();
    for arg in args {
        match arg.as_str() {
            "--json" => parsed.json = true,
            _ => bail!("unknown setup option: {arg}"),
        }
    }
    Ok(parsed)
}

pub(crate) fn parse_plugin_hook_args(args: &[String]) -> Result<PluginHookArgs> {
    let mut parsed = PluginHookArgs::default();
    for arg in args {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--no-repair" => parsed.no_repair = true,
            _ => bail!("unknown setup pluginhook option: {arg}"),
        }
    }
    Ok(parsed)
}

pub(crate) fn parse_compose_args(args: &[String]) -> Result<ComposeArgs> {
    let mut parsed = ComposeArgs::default();
    parse_compose_common(args, &mut parsed.target, &mut parsed.json)?;
    reject_unknown_compose_args("compose", args, &[])?;
    Ok(parsed)
}

pub(crate) fn parse_compose_mutation(
    args: &[String],
    destructive: bool,
) -> Result<ComposeMutationArgs> {
    let mut parsed = ComposeMutationArgs::default();
    parse_compose_common(args, &mut parsed.target, &mut parsed.json)?;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--dry-run" => parsed.options.dry_run = true,
            "--allow-cwd-target" => parsed.options.allow_cwd_target = true,
            "--yes" => parsed.options.yes = true,
            _ if is_compose_common_arg(&arg) => {
                consume_compose_common_value(&mut flags, &arg)?;
            }
            _ if arg.starts_with("--") => bail!("unknown compose option: {arg}"),
            _ => bail!("unexpected compose argument: {arg}"),
        }
    }
    parsed.options.non_interactive = destructive;
    Ok(parsed)
}

pub(crate) fn parse_compose_logs(args: &[String]) -> Result<ComposeLogsArgs> {
    let mut parsed = ComposeLogsArgs::default();
    parse_compose_common(args, &mut parsed.target, &mut parsed.json)?;
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--tail" => parsed.tail = Some(parse_u32_flag("--tail", flags.value("--tail")?)?),
            _ if arg.starts_with("--tail=") => {
                parsed.tail = Some(parse_u32_flag(
                    "--tail",
                    value_after_equals(arg, "--tail")?,
                )?)
            }
            "--follow" => bail!("cortex compose logs --follow is deferred"),
            _ if is_compose_common_arg(&arg) => {
                consume_compose_common_value(&mut flags, &arg)?;
            }
            _ if arg.starts_with("--") => bail!("unknown compose logs option: {arg}"),
            _ => bail!("unexpected compose logs argument: {arg}"),
        }
    }
    Ok(parsed)
}

fn parse_compose_logs_command(args: &[String]) -> Result<CliCommand> {
    if matches!(args.first(), Some(first) if !first.starts_with('-')) {
        return Ok(CliCommand::Compose(ComposeCommand::ServiceLogs(
            parse_service_logs_args("compose logs", args)?,
        )));
    }
    Ok(CliCommand::Compose(ComposeCommand::Logs(
        parse_compose_logs(args)?,
    )))
}

pub(crate) fn parse_compose_common(
    args: &[String],
    target: &mut ComposeTarget,
    json: &mut bool,
) -> Result<()> {
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => *json = true,
            "--compose-file" => target.compose_file = Some(flags.value("--compose-file")?.into()),
            "--project-dir" => target.project_dir = Some(flags.value("--project-dir")?.into()),
            "--project-name" => target.project_name = Some(flags.value("--project-name")?),
            "--service" => target.service = Some(flags.value("--service")?),
            "--container" => target.container_name = Some(flags.value("--container")?),
            _ if arg.starts_with("--compose-file=") => {
                target.compose_file = Some(value_after_equals(arg, "--compose-file")?.into())
            }
            _ if arg.starts_with("--project-dir=") => {
                target.project_dir = Some(value_after_equals(arg, "--project-dir")?.into())
            }
            _ if arg.starts_with("--project-name=") => {
                target.project_name = Some(value_after_equals(arg, "--project-name")?)
            }
            _ if arg.starts_with("--service=") => {
                target.service = Some(value_after_equals(arg, "--service")?)
            }
            _ if arg.starts_with("--container=") => {
                target.container_name = Some(value_after_equals(arg, "--container")?)
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn reject_unknown_compose_args(
    command: &str,
    args: &[String],
    extra: &[&str],
) -> Result<()> {
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if extra.contains(&arg.as_str()) {
            continue;
        }
        if is_compose_common_arg(&arg) {
            consume_compose_common_value(&mut flags, &arg)?;
            continue;
        }
        if arg.starts_with("--") {
            bail!("unknown {command} option: {arg}");
        }
        bail!("unexpected {command} argument: {arg}");
    }
    Ok(())
}

pub(crate) fn is_compose_common_arg(arg: &str) -> bool {
    matches!(
        arg,
        "--json"
            | "--compose-file"
            | "--project-dir"
            | "--project-name"
            | "--service"
            | "--container"
    ) || arg.starts_with("--compose-file=")
        || arg.starts_with("--project-dir=")
        || arg.starts_with("--project-name=")
        || arg.starts_with("--service=")
        || arg.starts_with("--container=")
}

pub(crate) fn needs_value(arg: &str) -> bool {
    matches!(
        arg,
        "--compose-file" | "--project-dir" | "--project-name" | "--service" | "--container"
    )
}

pub(crate) fn consume_compose_common_value(flags: &mut FlagCursor<'_>, arg: &str) -> Result<()> {
    if !arg.contains('=') && needs_value(arg) {
        let _ = flags.value(arg)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "parse_admin_tests.rs"]
mod tests;
