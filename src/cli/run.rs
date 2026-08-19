use anyhow::{Result, anyhow, bail};
use cortex::app::CortexService;

use super::args::{
    AlertsCommand, CliCommand, DbCommand, GraphCommand, IngestCommand, NotifyCommand, ShellCommand,
    ShellUserCommand, SigCommand, StateCommand, StatsCommand,
};
use super::dispatch;

/// Env var that opts a process into HTTP transport without passing `--http`.
/// Accepts `1` or `true` (case-insensitive). Any other value is treated as
/// unset to avoid surprising "I typoed `falze`" silent flips.
pub(crate) const ENV_USE_HTTP: &str = "CORTEX_USE_HTTP";

/// CLI transport mode resolved from global flags + env. Built once per
/// invocation; passed by value into [`run`].
///
/// `Local` keeps the full sqlx + rusqlite + FTS5 stack linked into the host
/// binary — acknowledged limitation, tracked for the v0.30 successor (bead
/// .12 doc note + epic acceptance criteria).
pub(crate) enum CliMode {
    Local(CortexService),
    Http(super::http_client::HttpClient),
}

impl std::fmt::Debug for CliMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(_) => f.write_str("CliMode::Local(CortexService)"),
            Self::Http(_) => f.write_str("CliMode::Http(HttpClient)"),
        }
    }
}

/// Top-level dispatch entry point. Built once per CLI invocation by `run_cli`
/// in `main.rs`. The [`CliMode`] decides whether we hit a local SQLite-backed
/// [`CortexService`] or a remote container via [`HttpClient`].
///
/// HTTP dispatch is implemented incrementally by bead .7+ — for now, the
/// `Http` arm returns a clear placeholder error per command. The mode wiring
/// is in place so .7 can light up commands one by one without touching this
/// signature.
pub(crate) async fn run(mode: CliMode, command: CliCommand) -> Result<()> {
    // Query commands (search/tail/errors/hosts/correlate/stats/sessions) are
    // mode-agnostic: dispatch::run_X branches on `&CliMode` internally and
    // wraps the HTTP path in `http_or_cancel` for SIGINT handling. Everything
    // else (ai/db/compose/setup) still flows through the Local-only path.
    match command {
        CliCommand::Search(args) => dispatch::run_search(&mode, args).await,
        CliCommand::Filter(args) => dispatch::run_filter(&mode, args).await,
        CliCommand::Tail(args) => dispatch::run_tail(&mode, args).await,
        CliCommand::Errors(args) => dispatch::run_errors(&mode, args).await,
        CliCommand::Hosts(command) => match command {
            super::HostsCommand::List(args) => dispatch::run_hosts(&mode, args).await,
            super::HostsCommand::Sources(args) => dispatch::run_source_ips(&mode, args).await,
            super::HostsCommand::Silent(args) => dispatch::run_silent_hosts(&mode, args).await,
        },
        CliCommand::Incident(args) => dispatch::run_incident(&mode, args).await,
        CliCommand::Correlate(args) => dispatch::run_correlate(&mode, args).await,
        CliCommand::State(command) => match command {
            StateCommand::Host(args) => dispatch::run_host_state(&mode, args).await,
            StateCommand::Fleet(args) => dispatch::run_fleet_state(&mode, args).await,
            StateCommand::ClockSkew(args) => dispatch::run_clock_skew(&mode, args).await,
        },
        CliCommand::Stats(command) => match command {
            StatsCommand::Summary(args) => dispatch::run_stats(&mode, args).await,
            StatsCommand::IngestRate(args) => dispatch::run_ingest_rate(&mode, args).await,
        },
        CliCommand::Status(args) => dispatch::run_status(&mode, args).await,
        CliCommand::Ingest(command) => match command {
            IngestCommand::Shell(shell) => match shell {
                ShellCommand::User(user) => match user {
                    ShellUserCommand::Index(args) => {
                        super::dispatch_command_log::run_shell_index(&mode, args).await
                    }
                    ShellUserCommand::AtuinIndex(args) => {
                        super::dispatch_command_log::run_shell_atuin_index(&mode, args).await
                    }
                },
                // Both `ShellAgentCommand` variants are intercepted in
                // `main.rs` before `CliMode`/`RuntimeCore` construction —
                // `Index` because it may forward instead of touching a local
                // DB, `Wrap` because of its liveness-probe fast path. Neither
                // should ever reach this generic dispatcher.
                ShellCommand::Agent(_) => {
                    bail!(
                        "internal: ingest shell agent commands must be dispatched before CliMode creation"
                    )
                }
            },
            IngestCommand::Inventory(_) => {
                bail!(
                    "internal: ingest inventory must be dispatched by main::run_cli before reaching cli::run()"
                )
            }
            IngestCommand::FileTail(command) => dispatch::run_file_tail(&mode, command).await,
            IngestCommand::SyslogStatus(args) => super::run_ingest_syslog_status(args).await,
            IngestCommand::DockerStatus(args) => super::run_ingest_docker_status(args).await,
            IngestCommand::DockerSources(args) => super::run_ingest_docker_sources(args).await,
        },
        CliCommand::Sessions(command) => match command {
            super::SessionsCommand::List(args) => dispatch::run_sessions(&mode, args).await,
            super::SessionsCommand::Search(args) => dispatch::run_ai_search(&mode, args).await,
            super::SessionsCommand::Abuse(args) => dispatch::run_ai_abuse(&mode, args).await,
            super::SessionsCommand::Correlate(args) => {
                dispatch::run_ai_correlate(&mode, args).await
            }
            super::SessionsCommand::Blocks(args) => dispatch::run_ai_blocks(&mode, args).await,
            super::SessionsCommand::Context(args) => dispatch::run_ai_context(&mode, args).await,
            super::SessionsCommand::Tools(args) => dispatch::run_ai_tools(&mode, args).await,
            super::SessionsCommand::Projects(args) => dispatch::run_ai_projects(&mode, args).await,
            super::SessionsCommand::Checkpoints(args) => {
                dispatch::run_ai_checkpoints(&mode, args).await
            }
            super::SessionsCommand::Errors(args) => dispatch::run_ai_errors(&mode, args).await,
            super::SessionsCommand::PruneCheckpoints(args) => {
                dispatch::run_ai_prune_checkpoints(&mode, args).await
            }
            super::SessionsCommand::Index(args) => dispatch::run_ai_index(&mode, args).await,
            super::SessionsCommand::Add(args) => dispatch::run_ai_add(&mode, args).await,
            super::SessionsCommand::Doctor(args) => dispatch::run_ai_doctor(&mode, args).await,
            super::SessionsCommand::SmokeWatch(args) => {
                dispatch::run_ai_smoke_watch(&mode, args).await
            }
            super::SessionsCommand::WatchStatus(args) => {
                dispatch::run_sessions_watch_status(&mode, args).await
            }
            super::SessionsCommand::Watch(args) => dispatch::run_sessions_watch(&mode, args).await,
            super::SessionsCommand::SimilarIncidents(args) => {
                dispatch::run_ai_similar_incidents(&mode, args).await
            }
            super::SessionsCommand::IncidentContext(args) => {
                dispatch::run_ai_incident_context(&mode, args).await
            }
            super::SessionsCommand::Incidents(args) => {
                dispatch::run_ai_incidents(&mode, args).await
            }
            super::SessionsCommand::Investigate(args) => {
                dispatch::run_ai_investigate(&mode, args).await
            }
            super::SessionsCommand::Assess(args) => dispatch::run_ai_assess(&mode, args).await,
            super::SessionsCommand::LlmInvocations(args) => {
                dispatch::run_ai_llm_invocations(&mode, args).await
            }
            super::SessionsCommand::Skills(args) => dispatch::run_ai_skills(&mode, args).await,
            super::SessionsCommand::HookEvents(args) => {
                dispatch::run_ai_hook_events(&mode, args).await
            }
            super::SessionsCommand::HookIncidents(args) => {
                dispatch::run_ai_hook_incidents(&mode, args).await
            }
            super::SessionsCommand::HookInvestigate(args) => {
                dispatch::run_ai_hook_investigate(&mode, args).await
            }
            super::SessionsCommand::HooksBackfill(args) => {
                dispatch::run_ai_hooks_backfill(&mode, args).await
            }
            super::SessionsCommand::SkillsBackfill(args) => {
                dispatch::run_ai_skills_backfill(&mode, args).await
            }
            super::SessionsCommand::SkillIncidents(args) => {
                dispatch::run_ai_skill_incidents(&mode, args).await
            }
            super::SessionsCommand::SkillInvestigate(args) => {
                dispatch::run_ai_skill_investigate(&mode, args).await
            }
            super::SessionsCommand::SkillAssess(args) => {
                dispatch::run_assess_skill(&mode, args).await
            }
            super::SessionsCommand::McpEvents(args) => dispatch::run_mcp_events(&mode, args).await,
            super::SessionsCommand::McpEventsBackfill(args) => {
                dispatch::run_mcp_events_backfill(&mode, args).await
            }
            super::SessionsCommand::McpIncidents(args) => {
                dispatch::run_mcp_incidents(&mode, args).await
            }
            super::SessionsCommand::McpInvestigate(args) => {
                dispatch::run_mcp_investigate(&mode, args).await
            }
            super::SessionsCommand::McpAssess(args) => dispatch::run_assess_mcp(&mode, args).await,
        },
        CliCommand::Assess(command) => match command {
            super::AssessCommand::Skill(args) => dispatch::run_assess_skill(&mode, args).await,
            super::AssessCommand::Abuse(args) => dispatch::run_assess_abuse(&mode, args).await,
            super::AssessCommand::Mcp(args) => dispatch::run_assess_mcp(&mode, args).await,
            super::AssessCommand::Hooks(args) => dispatch::run_assess_hooks(&mode, args).await,
        },
        CliCommand::Heartbeat(command) => {
            super::heartbeat_agent::run_heartbeat_no_db(command).await
        }
        // DB commands (bead 0p8r.9). 4 are HTTP-capable; backup stays LOCAL
        // and bails in HTTP mode with an inline message.
        CliCommand::Db(db) => match db {
            DbCommand::Status(args) => dispatch::run_db_status(&mode, args).await,
            DbCommand::Integrity(args) => dispatch::run_db_integrity(&mode, args).await,
            DbCommand::IntegrityStatus(args) => {
                dispatch::run_db_integrity_status(&mode, args).await
            }
            DbCommand::Checkpoint(args) => dispatch::run_db_checkpoint(&mode, args).await,
            DbCommand::Vacuum(args) => dispatch::run_db_vacuum(&mode, args).await,
            DbCommand::Backup(args) => dispatch::run_db_backup(&mode, args).await,
        },
        // Compose/Setup/Config are local-only and main::run_cli reroutes them BEFORE
        // calling run(). If we reach here, the front door was bypassed —
        // bail with a clear internal-error message rather than a placeholder.
        CliCommand::Timeline(args) => dispatch::run_timeline(&mode, args).await,
        CliCommand::Patterns(args) => dispatch::run_patterns(&mode, args).await,
        CliCommand::Alerts(alerts) => match alerts {
            AlertsCommand::Signatures(sig) => match sig {
                SigCommand::List(args) => dispatch::run_sig_list(&mode, args).await,
                SigCommand::Ack(args) => dispatch::run_sig_ack(&mode, args).await,
                SigCommand::Unack(args) => dispatch::run_sig_unack(&mode, args).await,
            },
            AlertsCommand::Notifications(notify) => match notify {
                NotifyCommand::Recent(args) => dispatch::run_notify_recent(&mode, args).await,
                NotifyCommand::Test(args) => dispatch::run_notify_test(&mode, args).await,
            },
        },
        // Surface parity gap closure (2026-05-22)
        CliCommand::Anomalies(args) => dispatch::run_anomalies(&mode, args).await,
        CliCommand::Compare(args) => dispatch::run_compare(&mode, args).await,
        CliCommand::Apps(args) => dispatch::run_apps(&mode, args).await,
        // Heartbeat fleet state parity (cxih.4)
        CliCommand::CorrelateState(args) => dispatch::run_correlate_state(&mode, args).await,
        CliCommand::TopicCorrelate(args) => dispatch::run_topic_correlate(&mode, args).await,
        CliCommand::Entity(args) => dispatch::run_entity_lookup(&mode, args).await,
        CliCommand::Graph(graph) => match graph {
            GraphCommand::Around(args) => dispatch::run_graph_around(&mode, args).await,
            GraphCommand::Explain(args) => dispatch::run_graph_explain(&mode, args).await,
            GraphCommand::Evidence(args) => dispatch::run_graph_evidence(&mode, args).await,
            GraphCommand::Status(args) => dispatch::run_graph_status(&mode, args).await,
            GraphCommand::Rebuild(args) => dispatch::run_graph_rebuild(&mode, args).await,
        },
        CliCommand::Compose(_) | CliCommand::Setup(_) => {
            bail!(
                "internal: compose/setup must be dispatched by main::run_cli before reaching cli::run()"
            )
        }
        CliCommand::Config(_) => {
            bail!(
                "internal: config commands must be dispatched by main::run_cli before reaching cli::run()"
            )
        }
        CliCommand::Complete(_) | CliCommand::Completions(_) => {
            bail!(
                "internal: completion commands must be dispatched by main::run_cli before reaching cli::run()"
            )
        }
    }
}

/// Global CLI flags that apply to every subcommand. Stripped from the raw
/// arg list by [`GlobalFlags::extract`] **before** subcommand parsing so the
/// per-command parsers (which we did NOT touch in this bead) keep matching
/// only the flags they already know about.
///
/// `--http` is a bare bool. `--server` and `--token` accept either a separate
/// arg (`--server URL`) or `=`-glued form (`--server=URL`). Passing `--server`
/// or `--token` implies HTTP mode even without an explicit `--http`.
///
/// `CORTEX_API_TOKEN` alone does **not** flip the default to HTTP — that
/// would silently change behaviour for users who already exported the token
/// from earlier deploys (locked decision, eng-review #C6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GlobalFlags {
    pub force_http: bool,
    pub server: Option<String>,
    pub token: Option<String>,
}

impl GlobalFlags {
    /// Strip global flags out of `args` in-place and return them.
    ///
    /// Unknown args are left in place untouched so the existing per-subcommand
    /// parsers see exactly what they used to. We deliberately allow both
    /// `cortex --http search foo` and `cortex search --http foo` — the
    /// stripper walks the command args until a `--` wrapped-command sentinel.
    ///
    /// `--server` / `--token` without a following value error out; an empty
    /// value (e.g. `--token=`) is also an error so a stray trailing `=` does
    /// not silently produce HTTP mode with a blank token.
    pub(crate) fn extract(args: &mut Vec<String>) -> Result<Self> {
        let mut out = GlobalFlags::default();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--" {
                break;
            }
            // Two flag families: bare "--http", and value-bearing
            // "--server"/"--token" which accept "--flag VALUE" or "--flag=VALUE".
            let arg = args[i].as_str();
            // `--http=<url>` is a convenience shortcut: enable HTTP transport AND
            // set the server in one flag (curl-style). Bare `--http` stays a flag
            // with no value so `cortex --http search foo` keeps `search` as the
            // command rather than swallowing it as a URL.
            if let Some(value) = strip_eq_prefix(arg, "--http") {
                if value.is_empty() {
                    bail!(
                        "--http=<url> requires a value; use bare --http with --server <url>, or --http=<url>"
                    );
                }
                out.force_http = true;
                out.server = Some(value.to_string());
                args.remove(i);
                continue;
            }
            if arg == "--http" {
                out.force_http = true;
                args.remove(i);
                continue;
            }
            if let Some(value) = strip_eq_prefix(arg, "--server") {
                if value.is_empty() {
                    bail!("--server requires a value");
                }
                out.server = Some(value.to_string());
                args.remove(i);
                continue;
            }
            if arg == "--server" {
                if i + 1 >= args.len() {
                    bail!("--server requires a value");
                }
                let value = args.remove(i + 1);
                if value.trim().is_empty() {
                    bail!("--server requires a non-empty value");
                }
                out.server = Some(value);
                args.remove(i);
                continue;
            }
            if let Some(value) = strip_eq_prefix(arg, "--token") {
                if value.is_empty() {
                    bail!("--token requires a value");
                }
                out.token = Some(value.to_string());
                args.remove(i);
                continue;
            }
            if arg == "--token" {
                if i + 1 >= args.len() {
                    bail!("--token requires a value");
                }
                let value = args.remove(i + 1);
                if value.trim().is_empty() {
                    bail!("--token requires a non-empty value");
                }
                out.token = Some(value);
                args.remove(i);
                continue;
            }
            i += 1;
        }
        Ok(out)
    }

    /// Returns `Some(trigger_label)` if HTTP mode was requested via any of:
    /// `--http`, `--server`, `--token`, or `CORTEX_USE_HTTP=1|true`. Returns
    /// `None` for the default Local mode. The label is the literal flag the
    /// user passed, used verbatim in error messages.
    ///
    /// Note: `CORTEX_API_TOKEN` being set does NOT trigger HTTP — only the
    /// explicit opt-ins above do (locked decision).
    pub(crate) fn http_trigger(&self) -> Option<&'static str> {
        if let Some(flag) = self.http_flag_trigger() {
            return Some(flag);
        }
        if env_opts_into_http() {
            return Some("CORTEX_USE_HTTP=1");
        }
        None
    }

    /// Like [`http_trigger`] but only considers explicit command-line FLAGS,
    /// ignoring the `CORTEX_USE_HTTP` env var. Used by local-only commands
    /// (`compose`, `setup`) that must not bail just because operators have
    /// `CORTEX_USE_HTTP=true` written into `~/.cortex/.env`.
    pub(crate) fn http_flag_trigger(&self) -> Option<&'static str> {
        if self.force_http {
            return Some("--http");
        }
        if self.server.is_some() {
            return Some("--server");
        }
        if self.token.is_some() {
            return Some("--token");
        }
        None
    }

    /// Build an [`HttpClient`] from these flags. On discovery failure, wraps
    /// the underlying error with a prefix naming the trigger so the operator
    /// knows exactly which knob put them into HTTP mode — this is the
    /// fail-closed contract from eng-review #C6.
    pub(crate) fn build_http_client(
        &self,
        trigger: &'static str,
    ) -> Result<super::http_client::HttpClient> {
        super::http_client::HttpClient::discover(self.server.clone(), self.token.clone())
            .map_err(|err| anyhow!("HTTP mode requested via {trigger} but discovery failed: {err}"))
    }
}

/// Returns `true` when `CORTEX_USE_HTTP` is set to `1` or `true`
/// (case-insensitive). Any other value — including empty string, `0`, `false`,
/// or typos — is treated as unset.
fn env_opts_into_http() -> bool {
    match crate::env::var(ENV_USE_HTTP) {
        Ok(v) => {
            let v = v.trim();
            v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
}

/// If `arg` matches `flag=...` return the suffix; otherwise `None`.
pub(crate) fn strip_eq_prefix<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    arg.strip_prefix(flag)
        .and_then(|rest| rest.strip_prefix('='))
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
