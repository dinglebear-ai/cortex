use anyhow::{Result, bail};
use cortex::app::ServiceLogsRequest;
use cortex::compose::{
    CliDockerInspect, ComposeDefaults, ComposeMutation, ComposeService, ProcessRunner,
};

mod args;
mod args_config;
pub(crate) use args::{
    AssessAbuseArgs, AssessCommand, AssessHooksArgs, AssessMcpArgs, AssessSkillArgs, CliCommand,
    ComposeArgs, ComposeCommand, ComposeLogsArgs, ComposeMutationArgs, CorrelateArgs, DbBackupArgs,
    DbCheckpointArgs, DbCommand, DbIntegrityArgs, DbIntegrityStatusArgs, DbStatusArgs,
    DbVacuumArgs, EntityArgs, FileTailAddArgs, FileTailCommand, FileTailIdArgs, FileTailListArgs,
    FilterArgs, GraphAroundArgs, GraphCommand, GraphEvidenceArgs, GraphExplainArgs,
    GraphRebuildArgs, GraphStatusArgs, HeartbeatAgentArgs, HeartbeatCommand, HostsCommand,
    IncidentArgs, IngestCommand, IngestRateArgs, InventoryArgs, InventoryCommand, NotifyRecentArgs,
    NotifyTestArgs, OutputArgs, PatternsArgs, PluginHookArgs, SearchArgs, ServiceLogsArgs,
    SessionsAbuseArgs, SessionsAddArgs, SessionsArgs, SessionsAssessArgs, SessionsBlocksArgs,
    SessionsCheckpointsArgs, SessionsCommand, SessionsContextArgs, SessionsCorrelateArgs,
    SessionsDoctorArgs, SessionsErrorsArgs, SessionsHookEventsListArgs, SessionsHookIncidentsArgs,
    SessionsHookInvestigateArgs, SessionsHooksBackfillArgs, SessionsIncidentContextArgs,
    SessionsIncidentsArgs, SessionsIndexArgs, SessionsInvestigateArgs, SessionsListArgs,
    SessionsLlmInvocationsArgs, SessionsMcpEventsBackfillArgs, SessionsMcpEventsListArgs,
    SessionsMcpIncidentsArgs, SessionsMcpInvestigateArgs, SessionsOutputDetail,
    SessionsPruneCheckpointsArgs, SessionsSearchArgs, SessionsSimilarArgs,
    SessionsSkillIncidentsArgs, SessionsSkillInvestigateArgs, SessionsSkillsBackfillArgs,
    SessionsSkillsListArgs, SessionsWatchArgs, SetupArgs, SetupCommand, ShellAgentCommand,
    ShellAgentIndexArgs, ShellAgentWrapArgs, ShellAtuinIndexArgs, ShellCommand, ShellIndexArgs,
    ShellUserCommand, SigAckArgs, SigListArgs, SigUnackArgs, SilentHostsArgs, SourceIpsArgs,
    TailArgs, TimeRangeArgs, TimelineArgs,
};
#[cfg(test)]
pub(crate) use args::{StateCommand, StatsCommand};
pub(crate) use args_config::{
    ConfigCommand, ConfigGetArgs, ConfigListArgs, ConfigSetArgs, ConfigTarget, ConfigUnsetArgs,
};

mod commands;

mod run;
pub(crate) use dispatch_command_log::{
    run_shell_agent_index_local, run_shell_agent_index_remote, run_shell_agent_wrap,
};
#[cfg(test)]
pub(crate) use run::ENV_USE_HTTP;
pub(crate) use run::{CliMode, GlobalFlags, run};

mod argdefaults;
pub(crate) mod color;
mod complete;
mod completions;
mod config_cmd;
mod config_toml;
mod coordination;
mod dispatch_command_log;
mod heartbeat_agent;
pub(crate) mod help;
mod output;
mod parse;
mod parse_admin;
mod parse_command_log;
mod parse_common;
mod parse_config;
mod parse_logs;
mod sessions_watch;
mod setup;
mod sparkline;
mod suggest;

pub(crate) use config_cmd::run_config;
pub(crate) use heartbeat_agent::run_heartbeat_no_db;
pub(crate) use parse_common::{FlagCursor, norm_time, parse_i64_flag, parse_u32_flag};
pub(crate) use setup::install_self;
pub(crate) use setup::run_setup;

impl CliCommand {
    pub(crate) fn parse(args: Vec<String>) -> Result<Self> {
        parse::parse_command(args)
    }
}

// ── Registry facade: CLI paths ↔ ACTION_SPECS metadata. Used by completion
// and discoverability help while MCP action names remain stable wire contracts.

fn registry_action_key(cli_command: &str) -> String {
    match cli_command {
        "state host" => "host_state".into(),
        "state fleet" => "fleet_state".into(),
        "state clockskew" => "clock_skew".into(),
        "stats ingestrate" => "ingest_rate".into(),
        "correlate events" => "correlate".into(),
        "correlate state" => "correlate_state".into(),
        "correlate topic" => "topic_correlate".into(),
        "analysis errors" => "errors".into(),
        "analysis incident" => "incident_context".into(),
        "analysis patterns" => "patterns".into(),
        "analysis anomalies" => "anomalies".into(),
        "analysis compare" => "compare".into(),
        "sessions incidentcontext" => "incident_context".into(),
        "sessions llminvocations" => "llm_invocations".into(),
        "sessions skills" => "skill_events".into(),
        "sessions skillincidents" => "skill_incidents".into(),
        "sessions skillinvestigate" => "skill_investigate".into(),
        "sessions mcpevents" => "mcp_events".into(),
        "sessions mcpincidents" => "mcp_incidents".into(),
        "sessions mcpinvestigate" => "mcp_investigate".into(),
        "sessions hookevents" => "hook_events".into(),
        "sessions hookincidents" => "hook_incidents".into(),
        "sessions hookinvestigate" => "hook_investigate".into(),
        "sessions hooksbackfill" => "hook_events".into(),
        "ingest filetail" => "file_tails".into(),
        other => other.replace([' ', '-'], "_"),
    }
}

/// All CLI command names paired with their one-line description (empty when the
/// command has no `ACTION_SPECS` entry, e.g. grouping commands like `sessions`).
pub(crate) fn registry_actions() -> Vec<(&'static str, &'static str)> {
    cortex::surfaces::canonical_cli_roots()
        .map(|spec| {
            let cmd = spec.spelling;
            let desc = cortex::mcp::description_for(&cmd.replace('-', "_")).unwrap_or("");
            (cmd, desc)
        })
        .collect()
}

/// Canonical flag metadata for a CLI command (empty slice when none).
pub(crate) fn registry_flags(cli_command: &str) -> &'static [cortex::mcp::FlagSpec] {
    cortex::mcp::flags_for(&registry_action_key(cli_command)).unwrap_or(&[])
}

/// Copy-paste examples for a CLI command (empty slice when none).
pub(crate) fn registry_examples(cli_command: &str) -> &'static [&'static str] {
    cortex::mcp::examples_for(&registry_action_key(cli_command)).unwrap_or(&[])
}

/// Canonical flag a bare positional binds to for a CLI command (`None` = the
/// command takes no positional).
pub(crate) fn registry_positional(cli_command: &str) -> Option<&'static str> {
    cortex::mcp::positional_for(&registry_action_key(cli_command))
}

/// `cortex __complete <ctx> ...` — print shell-completion candidates to stdout.
pub(crate) fn run_complete(args: &[String]) -> Result<()> {
    for line in complete::complete(args)? {
        println!("{line}");
    }
    Ok(())
}

/// `cortex completions <shell>` — print a completion script to stdout.
pub(crate) fn run_completions(args: &[String]) -> Result<()> {
    let shell = args.first().map(|s| s.as_str()).unwrap_or("zsh");
    completions::print_completions(shell)
}

pub(crate) async fn run_compose(command: CliCommand) -> Result<()> {
    let CliCommand::Compose(command) = command else {
        bail!("run_compose called with non-compose command");
    };
    let service = ComposeService::new(CliDockerInspect, ProcessRunner, ComposeDefaults::default());
    match command {
        ComposeCommand::Status(args) => {
            let status = service.status(&args.target)?;
            output::ops::print_compose_status_response(&status, args.json)
        }
        ComposeCommand::Doctor(args) => {
            let status = service.status(&args.target)?;
            let coordination = coordination::run_coordination_phases();
            output::ops::print_compose_doctor_response(&status, &coordination, args.json)?;
            output::ops::ensure_doctor_coordination_ok(&coordination)?;
            cortex::compose::ensure_doctor_ready(&status)
        }
        ComposeCommand::Up(args) => output::ops::print_compose_command_response(
            &service.run_mutation(ComposeMutation::Up, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Down(args) => output::ops::print_compose_command_response(
            &service.run_mutation(ComposeMutation::Down, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Restart(args) => output::ops::print_compose_command_response(
            &service.run_mutation(ComposeMutation::Restart, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Pull(args) => output::ops::print_compose_command_response(
            &service.run_mutation(ComposeMutation::Pull, &args.target, &args.options)?,
            args.json,
        ),
        ComposeCommand::Logs(args) => {
            let output = service.logs(&args.target, args.tail)?;
            if args.json {
                output::common::print_json(&output)?;
            } else {
                print!("{}", output.stdout);
                eprint!("{}", output.stderr);
            }
            output::ops::ensure_command_success(&output)
        }
        ComposeCommand::ServiceLogs(args) => {
            let json = args.json;
            let report = cortex::app::run_service_logs(
                ServiceLogsRequest {
                    service: args.service,
                    since: args.since,
                    until: args.until,
                    tail: args.tail,
                },
                &cortex::app::SystemOsAdapter,
            )
            .await?;
            output::sessions::print_service_logs_response(&report, json)
        }
    }
}

pub(crate) async fn run_inventory(command: InventoryCommand) -> Result<()> {
    match command {
        InventoryCommand::Refresh(args) => {
            let report = cortex::inventory::refresh_inventory(
                cortex::inventory::InventoryConfig::from_env(),
            )
            .await?;
            if args.json {
                output::common::print_json(&report)
            } else {
                println!("inventory refresh: {}", report.status);
                println!("root: {}", report.root);
                println!("normalized: {}", report.normalized_path);
                println!("collection_state: {}", report.collection_state_path);
                for warning in report.warnings {
                    println!("warning: {warning}");
                }
                Ok(())
            }
        }
        InventoryCommand::Status(args) => {
            let status = cortex::inventory::inventory_status(
                &cortex::inventory::InventoryConfig::from_env(),
            );
            if args.json {
                output::common::print_json(&status)
            } else {
                println!("inventory status: {}", status.status);
                println!("root: {}", status.root);
                println!("normalized: {}", status.normalized_path);
                if let Some(generated_at) = status.generated_at {
                    println!("generated_at: {generated_at}");
                }
                println!("stale: {}", status.is_stale);
                for warning in status.warnings {
                    println!("warning: {warning}");
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct IngestSyslogStatus {
    bind_addr: String,
    host: String,
    port: u16,
    max_message_size: usize,
    batch_size: usize,
    flush_interval_ms: u64,
}

#[derive(Debug, serde::Serialize)]
struct IngestDockerStatus {
    legacy_central_pull_enabled: bool,
    configured_sources: usize,
    excluded_containers: usize,
    reconnect_initial_ms: u64,
    reconnect_max_ms: u64,
    host_local_agent_note: &'static str,
}

#[derive(Debug, serde::Serialize)]
struct IngestDockerSource {
    name: String,
    endpoint_configured: bool,
    allow_insecure_http: bool,
    excluded_containers: usize,
}

pub(crate) async fn run_ingest_syslog_status(args: OutputArgs) -> Result<()> {
    let config = cortex::config::Config::load_for_inspection()?;
    let receiver = &config.receiver;
    let status = IngestSyslogStatus {
        bind_addr: receiver.bind_addr(),
        host: receiver.host.clone(),
        port: receiver.port,
        max_message_size: receiver.max_message_size,
        batch_size: receiver.batch_size,
        flush_interval_ms: receiver.flush_interval,
    };
    if args.json {
        output::common::print_json(&status)
    } else {
        println!("syslog ingest: {}", status.bind_addr);
        println!("max_message_size: {}", status.max_message_size);
        println!("batch_size: {}", status.batch_size);
        println!("flush_interval_ms: {}", status.flush_interval_ms);
        Ok(())
    }
}

pub(crate) async fn run_ingest_docker_status(args: OutputArgs) -> Result<()> {
    let config = cortex::config::Config::load_for_inspection()?;
    let docker = &config.docker_ingest;
    let status = IngestDockerStatus {
        legacy_central_pull_enabled: docker.enabled,
        configured_sources: docker.hosts.len(),
        excluded_containers: docker.excluded_containers.len(),
        reconnect_initial_ms: docker.reconnect_initial_ms,
        reconnect_max_ms: docker.reconnect_max_ms,
        host_local_agent_note: "host-local cortex agents stream Docker logs from each host when enabled there",
    };
    if args.json {
        output::common::print_json(&status)
    } else {
        println!(
            "docker ingest: legacy_central_pull_enabled={}",
            status.legacy_central_pull_enabled
        );
        println!("configured_sources: {}", status.configured_sources);
        println!("excluded_containers: {}", status.excluded_containers);
        println!(
            "reconnect_ms: {}..{}",
            status.reconnect_initial_ms, status.reconnect_max_ms
        );
        println!("{}", status.host_local_agent_note);
        Ok(())
    }
}

pub(crate) async fn run_ingest_docker_sources(args: OutputArgs) -> Result<()> {
    let config = cortex::config::Config::load_for_inspection()?;
    let sources = config
        .docker_ingest
        .hosts
        .iter()
        .map(|host| IngestDockerSource {
            name: host.name.clone(),
            endpoint_configured: !host.base_url.trim().is_empty(),
            allow_insecure_http: host.allow_insecure_http,
            excluded_containers: host.excluded_containers.len(),
        })
        .collect::<Vec<_>>();
    if args.json {
        output::common::print_json(&sources)
    } else {
        if sources.is_empty() {
            println!("No legacy central-pull Docker sources configured.");
            return Ok(());
        }
        for source in &sources {
            println!(
                "{} endpoint_configured={} allow_insecure_http={} excluded_containers={}",
                source.name,
                source.endpoint_configured,
                source.allow_insecure_http,
                source.excluded_containers
            );
        }
        Ok(())
    }
}

#[cfg(test)]
use coordination::{
    DoctorCache, SystemctlEnv, canonicalize_with_warning, lookup_systemd_db_path,
    parse_systemctl_env_output, sessions_watch_coordination_phase,
};
#[cfg(test)]
use cortex::scanner::AiDoctorReport;
#[cfg(test)]
use output::common::truncate;
#[cfg(test)]
use output::ops::ensure_doctor_coordination_ok;
#[cfg(test)]
use output::sessions::ensure_ai_doctor_success;
#[cfg(test)]
use sessions_watch::smoke_watch_target;
#[cfg(test)]
use setup::{SetupPhase, SetupStatus};

mod dispatch;
mod dispatch_db;
mod dispatch_sessions;
mod http_client;

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
