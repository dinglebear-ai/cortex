use super::*;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

struct EnvGuard {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = crate::env::var_os(name);
        crate::env::set_test_var(name, value);
        Self { name, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => {
                crate::env::set_test_var(self.name, value);
            }
            None => {
                crate::env::remove_test_var(self.name);
            }
        }
    }
}

#[test]
fn parse_search_collects_query_and_filters() {
    let parsed = CliCommand::parse(strings(&[
        "search",
        "disk",
        "full",
        "--host",
        "nas",
        "--source=10.0.0.5:514",
        "--severity",
        "err",
        "--app=kernel",
        "--facility=auth",
        "--exclude-facility",
        "transcript",
        "--since",
        "2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
        "--received-since=2026-01-01T00:00:30Z",
        "--received-until",
        "2026-01-02T00:00:30Z",
        "--limit",
        "25",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Search(SearchArgs {
            query: Some("disk full".into()),
            grep: None,
            host: Some("nas".into()),
            source: Some("10.0.0.5:514".into()),
            severity: Some("err".into()),
            app: Some("kernel".into()),
            facility: Some("auth".into()),
            exclude_facility: Some("transcript".into()),
            since: Some("2026-01-01T00:00:00+00:00".into()),
            until: Some("2026-01-02T00:00:00+00:00".into()),
            received_since: Some("2026-01-01T00:00:30+00:00".into()),
            received_until: Some("2026-01-02T00:00:30+00:00".into()),
            limit: Some(25),
            json: true,
        })
    );
}

#[test]
fn parse_tail_binds_positional_to_host_with_explicit_count() {
    // Plan 3 semantics: a bare positional binds to --host; the result count
    // comes from -n/--limit (no longer a bare-number positional).
    let parsed = CliCommand::parse(strings(&["tail", "router", "-n", "10"])).unwrap();

    assert_eq!(
        parsed,
        CliCommand::Tail(TailArgs {
            n: Some(10),
            host: Some("router".into()),
            ..Default::default()
        })
    );
}

#[test]
fn parse_compose_service_logs_accepts_time_range_and_json() {
    let parsed = CliCommand::parse(strings(&[
        "compose",
        "logs",
        "cortex-sessions-watch",
        "--since",
        "2026-05-19 19:55:00",
        "--until=2026-05-19 20:05:00",
        "--tail",
        "50",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Compose(ComposeCommand::ServiceLogs(ServiceLogsArgs {
            service: "cortex-sessions-watch".into(),
            since: Some("2026-05-19 19:55:00".into()),
            until: Some("2026-05-19 20:05:00".into()),
            tail: Some(50),
            json: true,
        }))
    );
}

#[test]
fn parse_old_service_top_level_command_is_removed() {
    let err = CliCommand::parse(strings(&["service", "logs", "cortex"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "removed CLI command: service\n\nUse `cortex compose logs SERVICE`."
    );
}

#[test]
fn parse_incident_accepts_window_service_and_json() {
    let parsed = CliCommand::parse(strings(&[
        "analysis",
        "incident",
        "--around",
        "2026-05-20T04:00:00Z",
        "--minutes",
        "10",
        "--service",
        "cortex-sessions-watch",
        "--host",
        "devhost",
        "--limit",
        "25",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Incident(IncidentArgs {
            // `--around` is normalized to RFC3339 (`+00:00`) at parse time.
            around: "2026-05-20T04:00:00+00:00".into(),
            minutes: Some(10),
            service: Some("cortex-sessions-watch".into()),
            host: Some("devhost".into()),
            limit: Some(25),
            json: true,
        })
    );
}

#[test]
fn parse_correlate_allows_shared_reference_time_default() {
    let command = CliCommand::parse(strings(&["correlate", "events", "--limit", "5"])).unwrap();
    let CliCommand::Correlate(args) = command else {
        panic!("expected correlate")
    };
    assert!(args.reference_time.is_none());
}

#[test]
fn parse_correlate_accepts_reference_time_and_filters() {
    let parsed = CliCommand::parse(strings(&[
        "correlate",
        "events",
        "--reference-time=2026-01-01T00:00:00Z",
        "--window-minutes",
        "15",
        "--severity-min=warning",
        "--query",
        "timeout",
        "--limit=50",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Correlate(CorrelateArgs {
            // `--reference-time` is normalized to RFC3339 (`+00:00`) at parse time.
            reference_time: Some("2026-01-01T00:00:00+00:00".into()),
            window_minutes: Some(15),
            severity_min: Some("warning".into()),
            query: Some("timeout".into()),
            limit: Some(50),
            json: true,
            ..Default::default()
        })
    );
}

#[test]
fn parse_unknown_option_errors() {
    let err = CliCommand::parse(strings(&["stats", "--bad"])).unwrap_err();

    assert!(err.to_string().contains("unknown stats option"));
}

#[test]
fn refreshed_session_leaves_map_to_canonical_actions() {
    for (path, action) in [
        ("sessions skills", "skill_events"),
        ("sessions skillincidents", "skill_incidents"),
        ("sessions skillinvestigate", "skill_investigate"),
        ("sessions mcpevents", "mcp_events"),
        ("sessions mcpincidents", "mcp_incidents"),
        ("sessions mcpinvestigate", "mcp_investigate"),
        ("sessions hookevents", "hook_events"),
        ("sessions hookincidents", "hook_incidents"),
        ("sessions hookinvestigate", "hook_investigate"),
    ] {
        assert_eq!(
            registry_action_key(path),
            action,
            "action mapping for {path}"
        );
    }
}

#[tokio::test]
async fn run_compose_rejects_non_compose_commands_before_live_probes() {
    let error = run_compose(CliCommand::Stats(StatsCommand::Summary(
        OutputArgs::default(),
    )))
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("non-compose command"));
}

#[tokio::test]
async fn run_inventory_status_uses_cache_only_and_accepts_json_or_text_output() {
    let dir = tempfile::tempdir().unwrap();
    let inventory_dir = dir.path().join("inventory");
    let _home = EnvGuard::set("HOME", dir.path());
    let _inventory = EnvGuard::set("CORTEX_INVENTORY_DIR", &inventory_dir);

    run_inventory(InventoryCommand::Status(InventoryArgs { json: true }))
        .await
        .unwrap();
    run_inventory(InventoryCommand::Status(InventoryArgs { json: false }))
        .await
        .unwrap();
}

#[test]
fn parse_sessions_search_collects_filters() {
    let parsed = CliCommand::parse(strings(&[
        "sessions",
        "search",
        "auth failure",
        "--tool",
        "claude",
        "--project=/tmp/project",
        "--limit",
        "5",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::Search(SessionsSearchArgs {
            query: "auth failure".into(),
            project: Some("/tmp/project".into()),
            tool: Some("claude".into()),
            limit: Some(5),
            ..Default::default()
        }))
    );
}

#[test]
fn parse_sessions_top_level_is_removed() {
    let err = CliCommand::parse(strings(&["ai", "search", "auth failure"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "removed CLI command: ai\n\nUse `cortex sessions`."
    );
}

#[test]
fn parse_sessions_abuse_collects_filters_and_context_options() {
    let parsed = CliCommand::parse(strings(&[
        "sessions",
        "abuse",
        "--project=/tmp/project",
        "--tool",
        "codex",
        "--limit",
        "5",
        "--before=3",
        "--after",
        "4",
        "--term",
        "dang",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::Abuse(SessionsAbuseArgs {
            project: Some("/tmp/project".into()),
            tool: Some("codex".into()),
            limit: Some(5),
            before: Some(3),
            after: Some(4),
            terms: vec!["dang".into()],
            json: true,
            ..Default::default()
        }))
    );
}

#[test]
fn parse_sessions_correlate_collects_cross_reference_filters() {
    let parsed = CliCommand::parse(strings(&[
        "sessions",
        "correlate",
        "--project=/tmp/project",
        "--tool",
        "codex",
        "--session-id",
        "sess-1",
        "--ai-query",
        "deploy",
        "--log-query=error",
        "--host",
        "host-a",
        "--app",
        "dockerd",
        "--window-minutes",
        "15",
        "--severity-min=err",
        "--events-per-anchor",
        "12",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::Correlate(SessionsCorrelateArgs {
            project: Some("/tmp/project".into()),
            tool: Some("codex".into()),
            session_id: Some("sess-1".into()),
            ai_query: Some("deploy".into()),
            log_query: Some("error".into()),
            host: Some("host-a".into()),
            app: Some("dockerd".into()),
            window_minutes: Some(15),
            severity_min: Some("err".into()),
            events_per_anchor: Some(12),
            json: true,
            ..Default::default()
        }))
    );
}

#[test]
fn parse_sessions_context_requires_project() {
    let err = CliCommand::parse(strings(&["sessions", "context"])).unwrap_err();
    assert!(err.to_string().contains("requires PROJECT"));
}

#[test]
fn parse_sessions_add_requires_file() {
    let err = CliCommand::parse(strings(&["sessions", "add"])).unwrap_err();
    assert!(err.to_string().contains("requires a file path"));
}

#[test]
fn parse_sessions_watch_defaults() {
    let command = CliCommand::parse(strings(&["sessions", "watch"])).unwrap();
    assert_eq!(
        command,
        CliCommand::Sessions(SessionsCommand::Watch(SessionsWatchArgs {
            path: None,
            debounce_ms: 750,
            settle_ms: 500,
            max_retries: 5,
            no_initial_scan: false,
            json: false,
        }))
    );
}

#[test]
fn parse_sessions_watch_all_options() {
    let command = CliCommand::parse(strings(&[
        "sessions",
        "watch",
        "--path",
        "/tmp/transcripts",
        "--debounce-ms",
        "100",
        "--settle-ms=250",
        "--max-retries=7",
        "--no-initial-scan",
        "--json",
    ]))
    .unwrap();
    assert_eq!(
        command,
        CliCommand::Sessions(SessionsCommand::Watch(SessionsWatchArgs {
            path: Some("/tmp/transcripts".into()),
            debounce_ms: 100,
            settle_ms: 250,
            max_retries: 7,
            no_initial_scan: true,
            json: true,
        }))
    );
}

#[test]
fn parse_sessions_watch_rejects_zero_timing_values() {
    let err = CliCommand::parse(strings(&["sessions", "watch", "--debounce-ms", "0"])).unwrap_err();
    assert!(err.to_string().contains("positive integer"));
}

#[test]
fn parse_sessions_index_collects_reindex_controls() {
    let parsed = CliCommand::parse(strings(&[
        "sessions",
        "index",
        "--path=/tmp/session.jsonl",
        "--since",
        "2026-05-14T00:00:00Z",
        "--force",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::Index(SessionsIndexArgs {
            path: Some("/tmp/session.jsonl".into()),
            since: Some("2026-05-14T00:00:00+00:00".into()),
            force: true,
            json: true,
        }))
    );
}

#[test]
fn parse_sessions_checkpoints_collects_filters() {
    let parsed = CliCommand::parse(strings(&[
        "sessions",
        "checkpoints",
        "--errors",
        "--limit=25",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::Checkpoints(SessionsCheckpointsArgs {
            errors_only: true,
            missing_only: false,
            limit: Some(25),
            json: true,
        }))
    );
}

#[test]
fn parse_sessions_errors_collects_limit() {
    let parsed =
        CliCommand::parse(strings(&["sessions", "errors", "--limit", "10", "--json"])).unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::Errors(SessionsErrorsArgs {
            limit: Some(10),
            json: true,
        }))
    );
}

#[test]
fn parse_sessions_prune_checkpoints_defaults_to_missing_sources() {
    let parsed =
        CliCommand::parse(strings(&["sessions", "prunecheckpoints", "--dry-run"])).unwrap();
    let CliCommand::Sessions(SessionsCommand::PruneCheckpoints(args)) = parsed else {
        panic!("unexpected command")
    };
    assert!(args.missing_only);
    assert!(args.dry_run);
}

#[test]
fn parse_sessions_doctor_accepts_strict_permissions() {
    let parsed = CliCommand::parse(strings(&[
        "sessions",
        "doctor",
        "--strict-permissions",
        "--json",
    ]))
    .unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::Doctor(SessionsDoctorArgs {
            json: true,
            strict_permissions: true,
        }))
    );
}

#[test]
fn parse_sessions_watch_status_accepts_json() {
    let parsed = CliCommand::parse(strings(&["sessions", "watchstatus", "--json"])).unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::WatchStatus(OutputArgs { json: true }))
    );
}

#[test]
fn parse_sessions_smoke_watch_accepts_json() {
    let parsed = CliCommand::parse(strings(&["sessions", "smokewatch", "--json"])).unwrap();

    assert_eq!(
        parsed,
        CliCommand::Sessions(SessionsCommand::SmokeWatch(OutputArgs { json: true }))
    );
}

#[test]
fn smoke_watch_target_uses_codex_root_when_claude_is_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let codex_root = temp.path().join(".codex/sessions");
    std::fs::create_dir_all(&codex_root).unwrap();
    let doctor = AiDoctorReport {
        db_path: "/tmp/cortex.db".into(),
        db_schema_version: 15,
        db_last_migration_at: Some("2026-01-01T00:00:00Z".into()),
        known_schema_version: 15,
        schema_current: true,
        claude_root: transcript_root_status("/missing", false),
        codex_root: transcript_root_status(&codex_root.to_string_lossy(), true),
        gemini_root: transcript_root_status("/missing-gemini", false),
        checkpoint_count: 0,
        checkpoint_error_count: 0,
        missing_checkpoint_count: 0,
        imported_record_count: 0,
        parse_error_count: 0,
        newest_indexed_path: None,
        newest_indexed_at: None,
    };

    let target = smoke_watch_target(&doctor, "stamp", "session-1", "2026-05-15T00:00:00Z")
        .expect("codex root should be selected");

    assert_eq!(target.tool, "codex");
    assert_eq!(
        target.project,
        std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string()
    );
    assert!(target.transcript_path.starts_with(codex_root));
    assert!(target.body.contains("\"type\":\"session_meta\""));
    assert!(target.body.contains("\"type\":\"response_item\""));
}

#[test]
fn strict_ai_doctor_permissions_ignore_missing_roots() {
    let doctor = AiDoctorReport {
        db_path: "/tmp/cortex.db".into(),
        db_schema_version: 15,
        db_last_migration_at: Some("2026-01-01T00:00:00Z".into()),
        known_schema_version: 15,
        schema_current: true,
        claude_root: transcript_root_status("/missing", false),
        codex_root: transcript_root_status("/tmp/codex", true),
        gemini_root: transcript_root_status("/missing-gemini", false),
        checkpoint_count: 0,
        checkpoint_error_count: 0,
        missing_checkpoint_count: 0,
        imported_record_count: 0,
        parse_error_count: 0,
        newest_indexed_path: None,
        newest_indexed_at: None,
    };

    ensure_ai_doctor_success(&doctor, true).expect("missing roots should not fail strict mode");
}

fn transcript_root_status(path: &str, available: bool) -> cortex::scanner::TranscriptRootStatus {
    cortex::scanner::TranscriptRootStatus {
        path: path.to_string(),
        exists: available,
        readable: available,
        writable: available,
        owner_uid: None,
        owner_gid: None,
        mode: None,
        strict_ok: available,
    }
}

#[test]
fn parse_db_status_accepts_json() {
    let parsed = CliCommand::parse(strings(&["db", "status", "--json"])).unwrap();

    assert_eq!(
        parsed,
        CliCommand::Db(DbCommand::Status(DbStatusArgs {
            json: true,
            check_coord: false,
        }))
    );
}

#[test]
fn parse_db_checkpoint_accepts_modes() {
    let parsed =
        CliCommand::parse(strings(&["db", "checkpoint", "--mode=truncate", "--json"])).unwrap();

    assert_eq!(
        parsed,
        CliCommand::Db(DbCommand::Checkpoint(DbCheckpointArgs {
            mode: "truncate".into(),
            json: true,
        }))
    );
}

#[test]
fn parse_db_checkpoint_rejects_unknown_mode() {
    let err = CliCommand::parse(strings(&["db", "checkpoint", "--mode", "bogus"])).unwrap_err();
    assert!(err.to_string().contains("passive, full, restart, truncate"));
}

#[test]
fn parse_db_vacuum_and_backup_options() {
    let vacuum = CliCommand::parse(strings(&["db", "vacuum", "--pages", "250"])).unwrap();
    assert_eq!(
        vacuum,
        CliCommand::Db(DbCommand::Vacuum(DbVacuumArgs {
            full: false,
            pages: 250,
            force: false,
            json: false,
        }))
    );

    let vacuum_full_force =
        CliCommand::parse(strings(&["db", "vacuum", "--full", "--force", "--json"])).unwrap();
    assert_eq!(
        vacuum_full_force,
        CliCommand::Db(DbCommand::Vacuum(DbVacuumArgs {
            full: true,
            pages: 1000,
            force: true,
            json: true,
        }))
    );

    let backup = CliCommand::parse(strings(&[
        "db",
        "backup",
        "--output=/tmp/syslog-backups",
        "--json",
    ]))
    .unwrap();
    assert_eq!(
        backup,
        CliCommand::Db(DbCommand::Backup(DbBackupArgs {
            output: Some("/tmp/syslog-backups".into()),
            json: true,
        }))
    );
}

#[test]
fn truncate_is_utf8_safe_for_non_ascii_project_names() {
    let value = truncate("项目路径-alpha", 6);
    assert!(value.ends_with('…'));
    assert!(value.is_char_boundary(value.len()));
}

#[test]
fn parse_search_help_points_to_top_level_usage() {
    let err = CliCommand::parse(strings(&["search", "--help"])).unwrap_err();

    assert!(err.to_string().contains("cortex --help"));
}

#[test]
fn parse_compose_status_collects_target() {
    let parsed = CliCommand::parse(strings(&[
        "compose",
        "status",
        "--compose-file",
        "/tmp/docker-compose.yml",
        "--project-name=cortex",
        "--json",
    ]))
    .unwrap();
    match parsed {
        CliCommand::Compose(ComposeCommand::Status(args)) => {
            assert_eq!(
                args.target.compose_file.unwrap(),
                std::path::PathBuf::from("/tmp/docker-compose.yml")
            );
            assert_eq!(args.target.project_name.as_deref(), Some("cortex"));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_compose_upgrade_is_deferred() {
    let err = CliCommand::parse(strings(&["compose", "upgrade"])).unwrap_err();
    assert!(err.to_string().contains("deferred"));
}

#[test]
fn parse_compose_logs_follow_is_deferred() {
    let err = CliCommand::parse(strings(&["compose", "logs", "--follow"])).unwrap_err();
    assert!(err.to_string().contains("deferred"));
}

#[test]
fn parse_compose_down_collects_yes_and_dry_run() {
    let parsed = CliCommand::parse(strings(&["compose", "down", "--yes", "--dry-run"])).unwrap();
    match parsed {
        CliCommand::Compose(ComposeCommand::Down(args)) => {
            assert!(args.options.yes);
            assert!(args.options.dry_run);
            assert!(args.options.non_interactive);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_setup_plugin_hook_collects_json_and_no_repair() {
    let parsed =
        CliCommand::parse(strings(&["setup", "pluginhook", "--json", "--no-repair"])).unwrap();

    assert_eq!(
        parsed,
        CliCommand::Setup(SetupCommand::PluginHook(PluginHookArgs {
            json: true,
            no_repair: true,
        }))
    );
}

#[test]
fn parse_db_status_accepts_check_coord() {
    let parsed = CliCommand::parse(strings(&["db", "status", "--check-coord", "--json"])).unwrap();
    assert_eq!(
        parsed,
        CliCommand::Db(DbCommand::Status(DbStatusArgs {
            json: true,
            check_coord: true,
        }))
    );
}

#[test]
fn parse_db_status_rejects_unknown_flag() {
    let err = CliCommand::parse(strings(&["db", "status", "--bogus"])).unwrap_err();
    assert!(err.to_string().contains("unknown db status option"));
}

#[test]
fn systemctl_env_parses_values_containing_equals() {
    // Eng-review #A50 / security #39: values may legitimately contain `=`,
    // so the parser must use `split_once('=')`, not `split('=')`.
    let stdout = "Environment=KEY=value=with=equals OTHER=plain\nLoadState=loaded\n";
    let env = parse_systemctl_env_output(stdout);
    assert_eq!(env.inline.len(), 2);
    assert_eq!(env.inline[0].0, "KEY");
    assert_eq!(env.inline[0].1, "value=with=equals");
    assert_eq!(env.inline[1].0, "OTHER");
    assert_eq!(env.inline[1].1, "plain");
    assert!(!env.unit_missing);
}

#[test]
fn systemctl_env_marks_unit_missing_on_not_found_load_state() {
    let stdout = "LoadState=not-found\nEnvironment=\nEnvironmentFiles=\n";
    let env = parse_systemctl_env_output(stdout);
    assert!(env.unit_missing);
}

#[test]
fn systemctl_env_files_strips_ignore_errors_suffix() {
    let stdout =
        "Environment=\nEnvironmentFiles=/etc/foo (ignore_errors=no) /etc/bar (ignore_errors=yes)\n";
    let env = parse_systemctl_env_output(stdout);
    assert_eq!(env.files.len(), 2);
    assert_eq!(env.files[0], std::path::PathBuf::from("/etc/foo"));
    assert_eq!(env.files[1], std::path::PathBuf::from("/etc/bar"));
}

#[test]
fn lookup_systemd_db_path_prefers_inline_environment() {
    let env = SystemctlEnv {
        inline: vec![("CORTEX_DB_PATH".into(), "/inline/cortex.db".into())],
        files: vec![],
        unit_missing: false,
    };
    assert_eq!(
        lookup_systemd_db_path(&env).as_deref(),
        Some("/inline/cortex.db")
    );
}

#[test]
fn lookup_systemd_db_path_falls_back_to_environment_files() {
    let temp = tempfile::tempdir().unwrap();
    let env_file = temp.path().join("ai-watch.env");
    std::fs::write(
        &env_file,
        "# leading comment\nCORTEX_DB_PATH=/file/cortex.db\nOTHER=ignored\n",
    )
    .unwrap();
    let env = SystemctlEnv {
        inline: vec![],
        files: vec![env_file],
        unit_missing: false,
    };
    assert_eq!(
        lookup_systemd_db_path(&env).as_deref(),
        Some("/file/cortex.db")
    );
}

#[test]
fn lookup_systemd_db_path_skips_missing_files_without_panic() {
    let env = SystemctlEnv {
        inline: vec![],
        files: vec![std::path::PathBuf::from("/nonexistent/path-12345.env")],
        unit_missing: false,
    };
    assert!(lookup_systemd_db_path(&env).is_none());
}

#[test]
fn canonicalize_with_warning_reports_enoent_instead_of_silent_compare() {
    // Eng-review #A48 / C5: the original drift bug fell back to literal
    // string compare when canonicalize failed. This test pins down the new
    // behaviour: ENOENT bubbles up as a structured warning string.
    let missing = std::path::PathBuf::from("/nonexistent-canon-test-9f3e2d1c");
    let err = canonicalize_with_warning(&missing).unwrap_err();
    assert!(err.contains("could not canonicalize"));
    assert!(err.contains("/nonexistent-canon-test-9f3e2d1c"));
}

#[test]
fn doctor_cache_dedupes_systemctl_show() {
    let mut cache = DoctorCache::default();
    let first = cache.systemctl_env("definitely-not-a-real.service-ab12cd");
    let second = cache.systemctl_env("definitely-not-a-real.service-ab12cd");
    // On any reasonable host this fake unit is either reported missing
    // (unit_missing == true) or the systemctl probe itself fails. Either
    // outcome is acceptable; a hit on the unit would mean the test host
    // genuinely has it installed, which we treat as a setup error.
    // Err(_) is also acceptable: systemctl unavailable / probe failed.
    if let Ok(env) = &first {
        assert!(env.unit_missing, "fake unit unexpectedly resolved: {env:?}");
    }
    // The cache returns clones of the same Result on the second call.
    match (&first, &second) {
        (Err(a), Err(b)) => assert_eq!(a, b),
        (Ok(a), Ok(b)) => assert_eq!(a.unit_missing, b.unit_missing),
        _ => panic!("cache returned divergent results for the same unit: {first:?} {second:?}"),
    }
}

#[test]
fn doctor_cache_dedupes_docker_inspect() {
    let mut cache = DoctorCache::default();
    let container = "definitely-not-a-real-container-ab12cd";
    let first = cache.container_inspect(container);
    let second = cache.container_inspect(container);
    match (&first, &second) {
        (Err(a), Err(b)) => assert_eq!(a, b),
        (Ok(a), Ok(b)) => {
            assert_eq!(a.running, b.running);
            assert_eq!(a.mount_source, b.mount_source);
        }
        _ => panic!("cache returned divergent results: {first:?} {second:?}"),
    }
}

#[test]
fn ai_watch_coordination_skipped_when_unit_missing() {
    // CORTEX_AI_WATCH_UNIT override forces the phase to query a unit that
    // cannot exist. On any reasonable test host this returns a LoadState
    // of not-found (Skipped per doctor spec) OR systemctl probe failure
    // (Warn). EnvVarGuard restores the in-memory test overlay on panic so
    // this test cannot leak CORTEX_AI_WATCH_UNIT into peers.
    let _g = EnvVarGuard::set(
        "CORTEX_AI_WATCH_UNIT",
        "cortex-sessions-watch-test-missing-9f3e.service",
    );
    let env_path = std::path::PathBuf::from("/nonexistent-env-9f3e");
    let mut cache = DoctorCache::default();
    let phase = sessions_watch_coordination_phase(&env_path, &mut cache);
    assert_eq!(phase.name, "sessions-watch-coord");
    assert!(
        matches!(phase.status, SetupStatus::Skipped | SetupStatus::Warn),
        "expected Skipped or Warn, got {:?} (detail={})",
        phase.status,
        phase.detail
    );
}

#[test]
fn ensure_doctor_coordination_ok_passes_with_only_warnings_or_skips() {
    let phases = vec![
        SetupPhase {
            name: "data-mount",
            status: SetupStatus::Skipped,
            detail: "no docker".into(),
        },
        SetupPhase {
            name: "ai-watch-coord",
            status: SetupStatus::Warn,
            detail: "could not canonicalize".into(),
        },
    ];
    assert!(ensure_doctor_coordination_ok(&phases).is_ok());
}

#[test]
fn ensure_doctor_coordination_ok_fails_on_error_phase() {
    let phases = vec![SetupPhase {
        name: "ai-watch-coord",
        status: SetupStatus::Error,
        detail: "paths diverged".into(),
    }];
    let err = ensure_doctor_coordination_ok(&phases).unwrap_err();
    assert!(err.to_string().contains("ai-watch-coord"));
    assert!(err.to_string().contains("paths diverged"));
}

#[test]
fn parse_setup_check_and_repair() {
    assert_eq!(
        CliCommand::parse(strings(&["setup", "check", "--json"])).unwrap(),
        CliCommand::Setup(SetupCommand::Check(SetupArgs { json: true }))
    );
    assert_eq!(
        CliCommand::parse(strings(&["setup", "repair"])).unwrap(),
        CliCommand::Setup(SetupCommand::Repair(SetupArgs { json: false }))
    );
}

// ─── GlobalFlags / CliMode (bead 0p8r.6) ────────────────────────────────────
//
// Env-touching tests use `#[serial]` (matching `http_client_tests`) so tests
// that intentionally share the same overlay keys cannot interfere semantically.
// The overlay itself never mutates the process environment.

use serial_test::serial;

/// Drop guard that restores the previous value of an env var when the test
/// exits — mirrors `EnvVarGuard` in `cli::http_client::tests`. Duplicated
/// (rather than re-exported) because that module is a sibling `mod` inside
/// `src/cli.rs` and tests live in this sibling — keeping the helper local
/// avoids tangling visibility just for tests.
struct EnvVarGuard {
    name: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let previous = crate::env::var(name).ok();
        crate::env::set_test_var(name, value);
        Self { name, previous }
    }
    fn unset(name: &'static str) -> Self {
        let previous = crate::env::var(name).ok();
        crate::env::remove_test_var(name);
        Self { name, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => crate::env::set_test_var(self.name, v),
            None => crate::env::remove_test_var(self.name),
        }
    }
}

#[test]
fn global_flags_default_is_empty() {
    let mut args = strings(&["search", "foo"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert_eq!(flags, GlobalFlags::default());
    assert_eq!(args, strings(&["search", "foo"]));
}

#[test]
fn global_flags_extract_http_bool_anywhere() {
    // Before the subcommand.
    let mut args = strings(&["--http", "search", "foo"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert!(flags.force_http);
    assert_eq!(args, strings(&["search", "foo"]));

    // After the subcommand.
    let mut args = strings(&["search", "--http", "foo"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert!(flags.force_http);
    assert_eq!(args, strings(&["search", "foo"]));

    // Trailing.
    let mut args = strings(&["search", "foo", "--http"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert!(flags.force_http);
    assert_eq!(args, strings(&["search", "foo"]));
}

#[test]
fn global_flags_extract_server_separate_and_eq_forms() {
    let mut args = strings(&["--server", "http://x:3100", "search", "foo"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert_eq!(flags.server.as_deref(), Some("http://x:3100"));
    assert_eq!(args, strings(&["search", "foo"]));

    let mut args = strings(&["search", "--server=http://y:3100", "foo"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert_eq!(flags.server.as_deref(), Some("http://y:3100"));
    assert_eq!(args, strings(&["search", "foo"]));
}

#[test]
fn global_flags_extract_token_separate_and_eq_forms() {
    let mut args = strings(&["search", "--token", "value", "foo"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert_eq!(flags.token.as_deref(), Some("value"));
    assert_eq!(args, strings(&["search", "foo"]));

    let mut args = strings(&["--token=value2", "search", "foo"]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert_eq!(flags.token.as_deref(), Some("value2"));
    assert_eq!(args, strings(&["search", "foo"]));
}

#[test]
fn global_flags_extract_rejects_missing_values() {
    assert!(GlobalFlags::extract(&mut strings(&["--server"])).is_err());
    assert!(GlobalFlags::extract(&mut strings(&["--token"])).is_err());
    assert!(GlobalFlags::extract(&mut strings(&["--server="])).is_err());
    assert!(GlobalFlags::extract(&mut strings(&["--token="])).is_err());
    // Trailing empty value via separate-arg form.
    assert!(GlobalFlags::extract(&mut strings(&["--server", ""])).is_err());
    assert!(GlobalFlags::extract(&mut strings(&["--token", "   "])).is_err());
}

#[test]
fn global_flags_combined_extract() {
    let mut args = strings(&[
        "--http",
        "sessions",
        "search",
        "--server=http://x:3100",
        "--token",
        "tok",
        "needle",
    ]);
    let flags = GlobalFlags::extract(&mut args).unwrap();
    assert!(flags.force_http);
    assert_eq!(flags.server.as_deref(), Some("http://x:3100"));
    assert_eq!(flags.token.as_deref(), Some("tok"));
    assert_eq!(args, strings(&["sessions", "search", "needle"]));
}

#[test]
#[serial]
fn http_trigger_default_no_env_no_flags_is_none() {
    let _g = EnvVarGuard::unset(ENV_USE_HTTP);
    let flags = GlobalFlags::default();
    assert_eq!(flags.http_trigger(), None);
}

#[test]
#[serial]
fn http_trigger_token_alone_does_not_imply_http_mode() {
    // The whole point of the locked decision: CORTEX_API_TOKEN being set
    // must NOT silently flip operators into HTTP mode just because they had
    // it exported from an earlier deploy.
    let _g1 = EnvVarGuard::unset(ENV_USE_HTTP);
    let _g2 = EnvVarGuard::set("CORTEX_API_TOKEN", "leftover-from-old-shell");
    let flags = GlobalFlags::default();
    assert_eq!(flags.http_trigger(), None);
}

#[test]
#[serial]
fn http_trigger_env_use_http_one_is_some() {
    let _g = EnvVarGuard::set(ENV_USE_HTTP, "1");
    let flags = GlobalFlags::default();
    assert_eq!(flags.http_trigger(), Some("CORTEX_USE_HTTP=1"));
}

#[test]
#[serial]
fn http_trigger_env_use_http_true_is_some() {
    let _g = EnvVarGuard::set(ENV_USE_HTTP, "TRUE");
    let flags = GlobalFlags::default();
    assert_eq!(flags.http_trigger(), Some("CORTEX_USE_HTTP=1"));
}

#[test]
#[serial]
fn http_trigger_env_use_http_other_values_are_none() {
    for v in ["0", "false", "no", "", "yes", "y", "FALZE", "anything"] {
        let _g = EnvVarGuard::set(ENV_USE_HTTP, v);
        let flags = GlobalFlags::default();
        assert_eq!(
            flags.http_trigger(),
            None,
            "value `{v}` should not opt into HTTP"
        );
    }
}

#[test]
#[serial]
fn http_trigger_http_flag_wins_over_env() {
    let _g = EnvVarGuard::unset(ENV_USE_HTTP);
    let flags = GlobalFlags {
        force_http: true,
        ..Default::default()
    };
    assert_eq!(flags.http_trigger(), Some("--http"));
}

#[test]
#[serial]
fn http_trigger_server_flag_implies_http() {
    let _g = EnvVarGuard::unset(ENV_USE_HTTP);
    let flags = GlobalFlags {
        server: Some("http://x:3100".into()),
        ..Default::default()
    };
    assert_eq!(flags.http_trigger(), Some("--server"));
}

#[test]
#[serial]
fn http_trigger_token_flag_implies_http() {
    let _g = EnvVarGuard::unset(ENV_USE_HTTP);
    let flags = GlobalFlags {
        token: Some("tok".into()),
        ..Default::default()
    };
    assert_eq!(flags.http_trigger(), Some("--token"));
}

#[test]
#[serial]
fn build_http_client_fails_closed_on_missing_token_via_http_flag() {
    // --http set but no token discoverable anywhere → must error with a
    // message naming --http (eng-review #C6).
    let _g1 = EnvVarGuard::unset(ENV_USE_HTTP);
    let _g2 = EnvVarGuard::unset("CORTEX_API_TOKEN");
    let _g3 = EnvVarGuard::unset("CORTEX_URL");
    let _g4 = EnvVarGuard::unset("CORTEX_PORT");
    let flags = GlobalFlags {
        force_http: true,
        ..Default::default()
    };
    let trigger = flags.http_trigger().expect("--http should trigger");
    let err = flags.build_http_client(trigger).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("HTTP mode requested via --http"),
        "expected '--http' in error, got: {msg}"
    );
    assert!(
        msg.contains("discovery failed"),
        "expected 'discovery failed' in error, got: {msg}"
    );
}

#[test]
#[serial]
fn build_http_client_fails_closed_on_missing_token_via_env() {
    let _g1 = EnvVarGuard::set(ENV_USE_HTTP, "1");
    let _g2 = EnvVarGuard::unset("CORTEX_API_TOKEN");
    let _g3 = EnvVarGuard::unset("CORTEX_URL");
    let _g4 = EnvVarGuard::unset("CORTEX_PORT");
    let flags = GlobalFlags::default();
    let trigger = flags.http_trigger().expect("env should trigger");
    let err = flags.build_http_client(trigger).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("HTTP mode requested via CORTEX_USE_HTTP=1"),
        "expected 'CORTEX_USE_HTTP=1' in error, got: {msg}"
    );
}

#[test]
#[serial]
fn build_http_client_fails_closed_on_missing_token_via_server_flag() {
    let _g1 = EnvVarGuard::unset(ENV_USE_HTTP);
    let _g2 = EnvVarGuard::unset("CORTEX_API_TOKEN");
    let flags = GlobalFlags {
        server: Some("http://otherhost:3100".into()),
        ..Default::default()
    };
    let trigger = flags.http_trigger().expect("--server should trigger");
    let err = flags.build_http_client(trigger).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("HTTP mode requested via --server"),
        "expected '--server' in error, got: {msg}"
    );
}

#[test]
#[serial]
fn build_http_client_succeeds_with_token_override() {
    // No env credential, but the flag override is supplied, so discovery succeeds and we get a
    // real HttpClient (we don't actually call it; just confirm it constructs).
    let _g1 = EnvVarGuard::unset(ENV_USE_HTTP);
    let _g2 = EnvVarGuard::unset("CORTEX_API_TOKEN");
    let _g3 = EnvVarGuard::unset("CORTEX_URL");
    let flags = GlobalFlags {
        token: Some("supplied-value".into()),
        ..Default::default()
    };
    let trigger = flags.http_trigger().expect("--token should trigger");
    let _client = flags.build_http_client(trigger).expect("should construct");
}

#[test]
fn setup_report_phase_list_does_not_include_data_mount_post_cutover() {
    // Bead cortex-0p8r.11: post-cutover (CORTEX_USE_HTTP=true is the default),
    // the SessionStart hook no longer needs to docker-inspect the container —
    // CLI no longer opens SQLite directly. Drift detection moved to
    // `compose doctor` (always) and `db status --check-coord` (opt-in) per
    // bead cortex-0p8r.13.
    //
    // Source-grep guard: ensure setup_report's push-list does not call
    // data_mount_phase. The function itself is intentionally retained because
    // compose doctor / db status --check-coord still use it.
    let source = include_str!("cli/setup.rs");

    // Find the setup_report fn body.
    let start = source
        .find("fn setup_report(mode: SetupMode)")
        .expect("setup_report fn signature should exist in cli/setup.rs");
    // Take the next ~5000 chars — generous bound around the fn body.
    let window_end = (start + 5000).min(source.len());
    let body = &source[start..window_end];

    assert!(
        !body.contains("phases.push(data_mount_phase("),
        "setup_report must NOT push data_mount_phase post-cutover \
         (bead cortex-0p8r.11). Use compose doctor / db status --check-coord \
         instead. Found a push call within the setup_report window."
    );
}

// ─── cortex-kmib: AI abuse incident investigations ───────────────────────

#[test]
fn parse_sessions_incidents_defaults() {
    let cmd = CliCommand::parse(strings(&["sessions", "incidents"])).unwrap();
    let CliCommand::Sessions(SessionsCommand::Incidents(args)) = cmd else {
        panic!("expected Incidents");
    };
    assert_eq!(args.project, None);
    assert_eq!(args.tool, None);
    assert_eq!(args.limit, None);
    assert_eq!(args.window_minutes, None);
    assert!(args.terms.is_empty());
    assert!(!args.json);
}

#[test]
fn parse_sessions_incidents_all_flags() {
    let cmd = CliCommand::parse(strings(&[
        "sessions",
        "incidents",
        "--project",
        "axon_rust",
        "--tool",
        "claude",
        "--limit",
        "5",
        "--window-minutes",
        "15",
        "--term",
        "shit",
        "--term",
        "fuck",
        "--json",
    ]))
    .unwrap();
    let CliCommand::Sessions(SessionsCommand::Incidents(args)) = cmd else {
        panic!("expected Incidents");
    };
    assert_eq!(args.project.as_deref(), Some("axon_rust"));
    assert_eq!(args.tool.as_deref(), Some("claude"));
    assert_eq!(args.limit, Some(5));
    assert_eq!(args.window_minutes, Some(15));
    assert_eq!(args.terms, vec!["shit", "fuck"]);
    assert!(args.json);
}

#[test]
fn parse_sessions_incidents_equals_syntax() {
    let cmd = CliCommand::parse(strings(&[
        "sessions",
        "incidents",
        "--project=lab",
        "--window-minutes=30",
        "--term=broken",
    ]))
    .unwrap();
    let CliCommand::Sessions(SessionsCommand::Incidents(args)) = cmd else {
        panic!("expected Incidents");
    };
    assert_eq!(args.project.as_deref(), Some("lab"));
    assert_eq!(args.window_minutes, Some(30));
    assert_eq!(args.terms, vec!["broken"]);
}

#[test]
fn parse_sessions_investigate_defaults() {
    let cmd = CliCommand::parse(strings(&["sessions", "investigate"])).unwrap();
    let CliCommand::Sessions(SessionsCommand::Investigate(args)) = cmd else {
        panic!("expected Investigate");
    };
    assert_eq!(args.project, None);
    assert_eq!(args.correlation_window_minutes, None);
    assert!(!args.json);
}

#[test]
fn parse_sessions_investigate_all_flags() {
    let cmd = CliCommand::parse(strings(&[
        "sessions",
        "investigate",
        "--project",
        "lab",
        "--window-minutes",
        "10",
        "--correlation-window-minutes",
        "20",
        "--limit",
        "3",
        "--term",
        "broken",
        "--json",
    ]))
    .unwrap();
    let CliCommand::Sessions(SessionsCommand::Investigate(args)) = cmd else {
        panic!("expected Investigate");
    };
    assert_eq!(args.project.as_deref(), Some("lab"));
    assert_eq!(args.window_minutes, Some(10));
    assert_eq!(args.correlation_window_minutes, Some(20));
    assert_eq!(args.limit, Some(3));
    assert_eq!(args.terms, vec!["broken"]);
    assert!(args.json);
}

#[test]
fn parse_sessions_investigate_equals_syntax() {
    let cmd = CliCommand::parse(strings(&[
        "sessions",
        "investigate",
        "--correlation-window-minutes=45",
    ]))
    .unwrap();
    let CliCommand::Sessions(SessionsCommand::Investigate(args)) = cmd else {
        panic!("expected Investigate");
    };
    assert_eq!(args.correlation_window_minutes, Some(45));
}

#[test]
fn parse_sessions_assess_with_incident_id() {
    let cmd = CliCommand::parse(strings(&["sessions", "assess", "inc-00000000deadbeef"])).unwrap();
    let CliCommand::Sessions(SessionsCommand::Assess(args)) = cmd else {
        panic!("expected Assess");
    };
    assert_eq!(args.incident_id, "inc-00000000deadbeef");
    assert_eq!(args.model, None);
    assert!(!args.json);
}

#[test]
fn parse_sessions_assess_with_model_and_json() {
    let cmd = CliCommand::parse(strings(&[
        "sessions",
        "assess",
        "inc-abc123",
        "--model",
        "gemini-2.0-flash",
        "--json",
    ]))
    .unwrap();
    let CliCommand::Sessions(SessionsCommand::Assess(args)) = cmd else {
        panic!("expected Assess");
    };
    assert_eq!(args.incident_id, "inc-abc123");
    assert_eq!(args.model.as_deref(), Some("gemini-2.0-flash"));
    assert!(args.json);
}

#[test]
fn parse_sessions_assess_requires_incident_id() {
    let result = CliCommand::parse(strings(&["sessions", "assess"]));
    assert!(result.is_err(), "assess without incident_id should fail");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("incident_id"),
        "error should mention incident_id, got: {msg}"
    );
}

#[test]
fn parse_sessions_assess_rejects_extra_positional() {
    let result = CliCommand::parse(strings(&["sessions", "assess", "inc-abc", "extra"]));
    assert!(
        result.is_err(),
        "assess with two positional args should fail"
    );
}

#[test]
fn parse_sessions_assess_filter_flags_forwarded() {
    // Verify that the same filter flags available on `ai incidents` are
    // also accepted on `ai assess` so the incident_id hash can be reproduced.
    let cmd = CliCommand::parse(strings(&[
        "sessions",
        "assess",
        "inc-abc123",
        "--project",
        "axon_rust",
        "--tool",
        "claude",
        "--window-minutes",
        "10",
        "--term",
        "shit",
        "--term",
        "broken",
        "--limit",
        "300",
    ]))
    .unwrap();
    let CliCommand::Sessions(SessionsCommand::Assess(args)) = cmd else {
        panic!("expected Assess");
    };
    assert_eq!(args.incident_id, "inc-abc123");
    assert_eq!(args.project.as_deref(), Some("axon_rust"));
    assert_eq!(args.tool.as_deref(), Some("claude"));
    assert_eq!(args.window_minutes, Some(10));
    assert_eq!(args.terms, vec!["shit", "broken"]);
    assert_eq!(args.limit, Some(300));
}

// ── Surface parity gap closure (2026-05-22) ─────────────────────────────────

#[test]
fn parse_hosts_silent_with_minutes_and_json() {
    let cmd = CliCommand::parse(strings(&[
        "hosts",
        "silent",
        "--silent-minutes",
        "120",
        "--json",
    ]))
    .expect("parse hosts silent");
    match cmd {
        CliCommand::Hosts(HostsCommand::Silent(args)) => {
            assert_eq!(args.silent_minutes, Some(120));
            assert!(args.json);
        }
        other => panic!("expected Hosts::Silent, got {other:?}"),
    }
}

#[test]
fn parse_hosts_sources_with_limit_offset_and_json() {
    let cmd = CliCommand::parse(strings(&[
        "hosts",
        "sources",
        "--limit",
        "10",
        "--offset=5",
        "--json",
    ]))
    .expect("parse hosts sources");
    match cmd {
        CliCommand::Hosts(HostsCommand::Sources(args)) => {
            assert_eq!(args.limit, Some(10));
            assert_eq!(args.offset, Some(5));
            assert!(args.json);
        }
        other => panic!("expected Hosts::Sources, got {other:?}"),
    }
}

#[test]
fn parse_old_host_inventory_top_level_commands_are_removed() {
    for (command, replacement) in [
        ("source-ips", "cortex hosts sources"),
        ("silent-hosts", "cortex hosts silent"),
    ] {
        let err = CliCommand::parse(strings(&[command])).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("removed CLI command: {command}\n\nUse `{replacement}`.")
        );
    }
}

#[test]
fn parse_clock_skew_with_since() {
    let cmd = CliCommand::parse(strings(&[
        "state",
        "clockskew",
        "--since",
        "2026-05-20T00:00:00Z",
    ]))
    .expect("parse clockskew");
    match cmd {
        CliCommand::State(StateCommand::ClockSkew(args)) => {
            assert_eq!(args.since.as_deref(), Some("2026-05-20T00:00:00+00:00"));
            assert!(!args.json);
        }
        other => panic!("expected ClockSkew, got {other:?}"),
    }
}

#[test]
fn parse_anomalies_with_windows() {
    let cmd = CliCommand::parse(strings(&[
        "analysis",
        "anomalies",
        "--recent-minutes",
        "30",
        "--baseline-minutes",
        "720",
    ]))
    .expect("parse anomalies");
    match cmd {
        CliCommand::Anomalies(args) => {
            assert_eq!(args.recent_minutes, Some(30));
            assert_eq!(args.baseline_minutes, Some(720));
        }
        other => panic!("expected Anomalies, got {other:?}"),
    }
}

#[test]
fn parse_compare_with_ranges() {
    let cmd = CliCommand::parse(strings(&[
        "analysis",
        "compare",
        "--a-from",
        "2026-05-20T00:00:00Z",
        "--a-to",
        "2026-05-20T23:59:59Z",
        "--b-from",
        "2026-05-21T00:00:00Z",
        "--b-to",
        "2026-05-21T23:59:59Z",
    ]))
    .expect("parse compare");
    match cmd {
        CliCommand::Compare(args) => {
            assert_eq!(args.a_from.as_deref(), Some("2026-05-20T00:00:00+00:00"));
            assert_eq!(args.b_to.as_deref(), Some("2026-05-21T23:59:59+00:00"));
        }
        other => panic!("expected Compare, got {other:?}"),
    }
}

#[test]
fn parse_apps_with_hostname_limit() {
    let cmd = CliCommand::parse(strings(&["apps", "--host", "devhost", "--limit", "50"]))
        .expect("parse apps");
    match cmd {
        CliCommand::Apps(args) => {
            assert_eq!(args.host.as_deref(), Some("devhost"));
            assert_eq!(args.limit, Some(50));
        }
        other => panic!("expected Apps, got {other:?}"),
    }
}
