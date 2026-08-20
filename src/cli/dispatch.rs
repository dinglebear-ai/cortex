//! Per-arm dispatch for query commands (bead 0p8r.7).
//!
//! For each of the 7 query commands (search/tail/errors/hosts/correlate/
//! stats/sessions) we expose:
//!
//! - A `Cli*Args::into_request()` conversion that constructs the `*Request`
//!   struct shared by the service layer and the REST surface. Sharing the
//!   constructor between the Local and HTTP arms is what guards against
//!   per-arm field drift (eng-review #A37). The unit tests below pin the
//!   shape via `format!("{req:?}")` snapshots.
//! - A `run_X(mode, args)` free `async fn` that branches on [`CliMode`] and
//!   either calls the local [`CortexService`] directly or routes through
//!   [`HttpClient`]. The HTTP arm is wrapped in [`http_or_cancel`] so a
//!   SIGINT during a long-running request bails with `"interrupted"`
//!   (eng-review #A29). The Local arm is sync SQL — no cancellation needed.
//!
//! `--json` printing reuses the existing `print_*_response` formatters from
//! `super::*`, so output is byte-identical between modes: the HTTP path
//! proxies the same service the Local path would invoke server-side.

use anyhow::{Result, bail};
use cortex::app::{
    CorrelateEventsRequest, FileTailOp, FileTailRequest, FileTailResponse, FilterLogsRequest,
    GetErrorsRequest, IncidentRequest, ListSessionsRequest, SearchLogsRequest, TailLogsRequest,
};
use std::future::Future;

use super::output::logs::{
    print_correlate_response, print_errors_response, print_hosts_response, print_search_response,
    print_sessions_response, print_stats_response,
};
use super::output::sessions::print_incident_response;
use super::{
    CliMode, CorrelateArgs, FileTailCommand, FileTailIdArgs, FilterArgs, IncidentArgs, SearchArgs,
    SessionsArgs, TailArgs, TimeRangeArgs,
};

// ─── Arg → Request conversions ──────────────────────────────────────────────
//
// One per `Cli*Args` struct in scope. No `IntoRequest` trait — per locked
// decision (memo from the bead description), a trait with one impl per type
// would be premature. The free `into_request()` methods are simpler and
// individually inlinable.

/// Wrap literal user text (`--grep`) as a single FTS5 phrase so operators and
/// hyphens match literally. Embedded double-quotes are doubled per FTS5 syntax.
fn fts_phrase_literal(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

impl SearchArgs {
    pub(crate) fn into_request(self) -> SearchLogsRequest {
        // `--grep` wins over a raw query: literal text is wrapped as a safe FTS5
        // phrase so hyphens/operators match literally.
        let query = match self.grep {
            Some(ref text) => Some(fts_phrase_literal(text)),
            None => self.query,
        };
        SearchLogsRequest {
            query,
            host: self.host,
            source: self.source,
            severity: self.severity,
            app: self.app,
            facility: self.facility,
            exclude_facility: self.exclude_facility,
            process_id: None,
            since: self.since,
            until: self.until,
            received_since: self.received_since,
            received_until: self.received_until,
            limit: self.limit,
            source_kind: None,
            tool: None,
            project: None,
            session_id: None,
            container: None,
            docker_host: None,
            stream: None,
            event_action: None,
        }
    }
}

impl FilterArgs {
    pub(crate) fn into_request(self) -> FilterLogsRequest {
        FilterLogsRequest {
            host: self.host,
            source: self.source,
            severity: self.severity,
            app: self.app,
            facility: self.facility,
            exclude_facility: self.exclude_facility,
            process_id: self.process_id,
            since: self.since,
            until: self.until,
            received_since: self.received_since,
            received_until: self.received_until,
            limit: self.limit,
            source_kind: self.source_kind,
            tool: self.tool,
            project: self.project,
            session_id: self.session_id,
            container: self.container,
            docker_host: self.docker_host,
            stream: self.stream,
            event_action: self.event_action,
        }
    }
}

impl IncidentArgs {
    pub(crate) fn into_request(self) -> IncidentRequest {
        IncidentRequest {
            around: (!self.around.is_empty()).then_some(self.around),
            minutes: self.minutes,
            service: self.service,
            host: self.host,
            limit: self.limit,
        }
    }
}

impl TailArgs {
    pub(crate) fn into_request(self) -> TailLogsRequest {
        TailLogsRequest {
            host: self.host,
            source: self.source,
            app: self.app,
            severity_min: None,
            n: self.n,
        }
    }
}

impl TimeRangeArgs {
    pub(crate) fn into_errors_request(self) -> GetErrorsRequest {
        GetErrorsRequest {
            since: self.since,
            until: self.until,
            group_by: None,
            limit: self.limit,
        }
    }
}

impl SessionsArgs {
    pub(crate) fn into_request(self) -> ListSessionsRequest {
        ListSessionsRequest {
            project: self.project,
            tool: self.tool,
            host: self.host,
            since: self.since,
            until: self.until,
            limit: self.limit,
        }
    }
}

impl CorrelateArgs {
    pub(crate) fn into_request(self) -> CorrelateEventsRequest {
        CorrelateEventsRequest {
            reference_time: self.reference_time,
            window_minutes: self.window_minutes,
            severity_min: self.severity_min,
            host: self.host,
            source: self.source,
            query: self.query,
            limit: self.limit,
        }
    }
}

// ─── Cancellation helper ────────────────────────────────────────────────────

/// Wrap an HTTP call so SIGINT (`ctrl_c`) cancels the in-flight request and
/// bails with `"interrupted"` (eng-review #A29).
///
/// Indirects through [`http_or_cancel_with`] so unit tests can pass a
/// deterministic cancellation future instead of `tokio::signal::ctrl_c()`,
/// which is impractical to trigger from inside the test process.
pub(crate) async fn http_or_cancel<T>(fut: impl Future<Output = Result<T>>) -> Result<T> {
    http_or_cancel_with(fut, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// Test-visible variant of [`http_or_cancel`] that accepts an arbitrary
/// cancellation future. Production code calls the wrapper above; tests in
/// `dispatch_tests.rs` plug in `tokio::time::sleep(...)` so the cancel branch
/// is deterministic.
pub(crate) async fn http_or_cancel_with<T>(
    fut: impl Future<Output = Result<T>>,
    cancel: impl Future<Output = ()>,
) -> Result<T> {
    tokio::select! {
        r = fut => r,
        _ = cancel => bail!("interrupted"),
    }
}

// ─── Per-command dispatch ───────────────────────────────────────────────────

pub(crate) async fn run_search(mode: &CliMode, args: SearchArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.search_logs(req).await?,
        CliMode::Http(client) => http_or_cancel(client.search(&req)).await?,
    };
    print_search_response(&response, json)
}

pub(crate) async fn run_filter(mode: &CliMode, args: FilterArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.filter_logs(req).await?,
        CliMode::Http(client) => http_or_cancel(client.filter(&req)).await?,
    };
    print_search_response(&response, json)
}

pub(crate) async fn run_tail(mode: &CliMode, args: TailArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.tail_logs(req).await?,
        CliMode::Http(client) => http_or_cancel(client.tail(&req)).await?,
    };
    print_search_response(&response, json)
}

pub(crate) async fn run_errors(mode: &CliMode, args: TimeRangeArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_errors_request();
    let response = match mode {
        CliMode::Local(service) => service.analysis().errors(req).await?,
        CliMode::Http(client) => http_or_cancel(client.errors(&req)).await?,
    };
    print_errors_response(&response, json)
}

pub(crate) async fn run_hosts(mode: &CliMode, args: super::OutputArgs) -> Result<()> {
    let response = match mode {
        CliMode::Local(service) => service.hosts().list().await?,
        CliMode::Http(client) => http_or_cancel(client.hosts()).await?,
    };
    print_hosts_response(&response, args.json)
}

pub(crate) async fn run_incident(mode: &CliMode, args: IncidentArgs) -> Result<()> {
    let json = args.json;
    match mode {
        CliMode::Http(_) => bail!("incident reads host-local service logs; omit --http"),
        CliMode::Local(service) => {
            let response = service.analysis().incident(args.into_request()).await?;
            print_incident_response(&response, json)
        }
    }
}

pub(crate) async fn run_correlate(mode: &CliMode, args: CorrelateArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.correlate().events(req).await?,
        CliMode::Http(client) => http_or_cancel(client.correlate(&req)).await?,
    };
    print_correlate_response(&response, json)
}

pub(crate) async fn run_stats(mode: &CliMode, args: super::OutputArgs) -> Result<()> {
    let response = match mode {
        CliMode::Local(service) => service.stats().summary().await?,
        CliMode::Http(client) => http_or_cancel(client.stats()).await?,
    };
    print_stats_response(&response, args.json)
}

pub(crate) async fn run_status(mode: &CliMode, args: super::OutputArgs) -> Result<()> {
    match mode {
        CliMode::Local(service) => {
            // Local mode: call service methods directly for status
            let db_ok = service.health_check().await.is_ok();
            let db_maintenance = service.db_status().await.ok();
            let file_tail_statuses = service.file_tail_statuses_snapshot();
            let file_tail_blocked_count = file_tail_statuses
                .iter()
                .filter(|status| status.blocked_on_writer_since.is_some())
                .count();
            let degraded = db_ok && file_tail_blocked_count > 0;

            let status = if db_ok {
                if degraded { "degraded" } else { "ok" }
            } else {
                "error"
            };

            if args.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": status,
                        "db_ok": db_ok,
                        "db_maintenance": db_maintenance,
                        "file_tails": {
                            "blocked_count": file_tail_blocked_count,
                            "statuses": file_tail_statuses
                        }
                    })
                );
            } else {
                println!("Status: {}", status);
                println!("Database: {}", if db_ok { "ok" } else { "error" });
                if let Some(db_maintenance) = db_maintenance {
                    println!("DB maintenance: {:?}", db_maintenance);
                }
                if file_tail_blocked_count > 0 {
                    println!("File tails blocked: {}", file_tail_blocked_count);
                }
                println!("File tails: {} total", file_tail_statuses.len());
            }
            Ok(())
        }
        CliMode::Http(_) => {
            bail!(
                "status command is not yet available in HTTP mode. Use Local mode or run via MCP action=status"
            )
        }
    }
}

pub(crate) async fn run_sessions(mode: &CliMode, args: SessionsArgs) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.list_sessions(req).await?,
        CliMode::Http(client) => http_or_cancel(client.sessions(&req)).await?,
    };
    print_sessions_response(&response, json)
}

pub(crate) async fn run_file_tail(mode: &CliMode, command: FileTailCommand) -> Result<()> {
    let (req, json) = match command {
        FileTailCommand::List(args) => (FileTailRequest::list(), args.json),
        FileTailCommand::Status(args) => (FileTailRequest::status(), args.json),
        FileTailCommand::Add(args) => {
            let req = FileTailRequest {
                op: FileTailOp::Add,
                id: args.id,
                path: Some(args.path),
                tag: args.tag,
                host: args.host,
                facility: args.facility,
                severity: args.severity,
                start_at_end: Some(args.start_at_end),
            };
            (req, args.json)
        }
        FileTailCommand::Remove(args) => id_request(FileTailOp::Remove, args),
        FileTailCommand::Enable(args) => id_request(FileTailOp::Enable, args),
        FileTailCommand::Disable(args) => id_request(FileTailOp::Disable, args),
    };
    let response = match mode {
        CliMode::Local(service) => service.ingest().file_tails(req).await?,
        CliMode::Http(client) => http_or_cancel(client.file_tails(&req)).await?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        print!("{}", format_file_tail_response(&response));
    }
    Ok(())
}

fn format_file_tail_response(response: &FileTailResponse) -> String {
    let mut out = String::new();
    for source in &response.sources {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            source.id,
            if source.enabled {
                "enabled"
            } else {
                "disabled"
            },
            source.tag,
            source.path
        ));
    }
    for status in &response.statuses {
        let last_error = status.last_error.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "{}\t{}\t{}\n",
            status.id, status.running, last_error
        ));
    }
    out
}

fn id_request(op: FileTailOp, args: FileTailIdArgs) -> (FileTailRequest, bool) {
    (FileTailRequest::id_op(op, args.id), args.json)
}

mod artifact_evidence;
mod surface;

pub(crate) use self::artifact_evidence::run_artifact_evidence;
pub(crate) use self::surface::{
    run_anomalies, run_apps, run_clock_skew, run_compare, run_correlate_state, run_entity_lookup,
    run_fleet_state, run_graph_around, run_graph_evidence, run_graph_explain, run_graph_rebuild,
    run_graph_status, run_host_state, run_ingest_rate, run_notify_recent, run_notify_test,
    run_patterns, run_sig_ack, run_sig_list, run_sig_unack, run_silent_hosts, run_source_ips,
    run_timeline, run_topic_correlate,
};
pub(crate) use super::dispatch_db::{
    run_db_backup, run_db_checkpoint, run_db_integrity, run_db_integrity_status, run_db_status,
    run_db_vacuum,
};
pub(crate) use super::dispatch_sessions::{
    run_ai_abuse, run_ai_add, run_ai_assess, run_ai_blocks, run_ai_checkpoints, run_ai_context,
    run_ai_correlate, run_ai_doctor, run_ai_errors, run_ai_hook_events, run_ai_hook_incidents,
    run_ai_hook_investigate, run_ai_hooks_backfill, run_ai_incident_context, run_ai_incidents,
    run_ai_index, run_ai_investigate, run_ai_llm_invocations, run_ai_projects,
    run_ai_prune_checkpoints, run_ai_search, run_ai_similar_incidents, run_ai_skill_incidents,
    run_ai_skill_investigate, run_ai_skills, run_ai_skills_backfill, run_ai_smoke_watch,
    run_ai_tools, run_assess_abuse, run_assess_hooks, run_assess_mcp, run_assess_skill,
    run_mcp_events, run_mcp_events_backfill, run_mcp_incidents, run_mcp_investigate,
    run_sessions_watch, run_sessions_watch_status,
};

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
