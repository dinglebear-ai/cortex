use super::*;

#[test]
fn session_page_requires_exact_identity_and_accepts_cursor() {
    let err = parse_sessions_page(&strings(&["--session-id", "s"]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("requires --project, --tool, --session-id, and --host"));

    let command = parse_sessions_command(&strings(&[
        "page",
        "--project",
        "/repo",
        "--tool",
        "codex",
        "--session-id",
        "s",
        "--host",
        "mac",
        "--cursor",
        "cortex-session-v1:7",
        "--limit",
        "5",
        "--json",
    ]))
    .unwrap();
    let crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Page(args)) = command else {
        panic!("expected session page command");
    };
    assert_eq!(args.cursor.as_deref(), Some("cortex-session-v1:7"));
    assert_eq!(args.limit, Some(5));
    assert!(args.json);
}

#[test]
fn sessions_add_accepts_a_positional_file() {
    let command = parse_sessions_command(&["add".into(), "/tmp/session.jsonl".into()]);
    assert!(
        command.is_ok(),
        "positional sessions add failed: {command:?}"
    );
}

#[test]
fn parse_sessions_search_requires_query() {
    let args = strings(&["--project", "/repo"]);

    let err = parse_sessions_search(&args).unwrap_err().to_string();

    assert!(err.contains("requires a query"));
}

#[test]
fn parse_sessions_watch_rejects_zero_debounce() {
    let args = strings(&["--debounce-ms", "0"]);

    let err = parse_sessions_watch(&args).unwrap_err().to_string();

    assert!(err.contains("expects a positive integer"));
}

#[test]
fn parse_sessions_blocks_accepts_limit_and_detail() {
    let args = strings(&["--limit", "12", "--detail", "full", "--json"]);

    let command = parse_sessions_blocks(&args).unwrap();

    match command {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Blocks(args)) => {
            assert_eq!(args.limit, Some(12));
            assert_eq!(args.detail, crate::cli::SessionsOutputDetail::Full);
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_sessions_dispatches_status_and_smoke_watch_output_flags() {
    let status = parse_sessions_command(&strings(&["watchstatus", "--json"])).unwrap();
    match status {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::WatchStatus(args)) => {
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let smoke = parse_sessions_command(&strings(&["smokewatch", "--json"])).unwrap();
    match smoke {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SmokeWatch(args)) => {
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_sessions_search_abuse_and_correlate_accept_equals_forms() {
    let search = parse_sessions_search(&strings(&[
        "--project=/repo",
        "--tool=Edit",
        "--since=2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
        "--limit=9",
        "--json",
        "disk",
        "full",
    ]))
    .unwrap();
    match search {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Search(args)) => {
            assert_eq!(args.query, "disk full");
            assert_eq!(args.project.as_deref(), Some("/repo"));
            assert_eq!(args.tool.as_deref(), Some("Edit"));
            assert_eq!(args.limit, Some(9));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let abuse = parse_sessions_abuse(&strings(&[
        "--project=/repo",
        "--tool=Bash",
        "--since=2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
        "--limit=10",
        "--before=2",
        "--after=3",
        "--term=panic",
        "--term",
        "timeout",
    ]))
    .unwrap();
    match abuse {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Abuse(args)) => {
            assert_eq!(args.before, Some(2));
            assert_eq!(args.after, Some(3));
            assert_eq!(args.terms, vec!["panic", "timeout"]);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let correlate = parse_sessions_correlate(&strings(&[
        "--project=/repo",
        "--tool=Read",
        "--session-id=s1",
        "--ai-query=build",
        "--log-query=error",
        "--host=host1",
        "--source=10.0.0.8",
        "--app=cortex",
        "--since=2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
        "--window-minutes=15",
        "--severity-min=warn",
        "--limit=20",
        "--events-per-anchor=4",
    ]))
    .unwrap();
    match correlate {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Correlate(args)) => {
            assert_eq!(args.session_id.as_deref(), Some("s1"));
            assert_eq!(args.window_minutes, Some(15));
            assert_eq!(args.events_per_anchor, Some(4));
            assert_eq!(args.source.as_deref(), Some("10.0.0.8"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_sessions_inventory_and_indexing_commands_accept_flags() {
    let context =
        parse_sessions_context(&strings(&["--project=/repo", "--tool=Edit", "--limit=3"])).unwrap();
    match context {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Context(args)) => {
            assert_eq!(args.project, "/repo");
            assert_eq!(args.tool.as_deref(), Some("Edit"));
            assert_eq!(args.limit, Some(3));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let tools = parse_sessions_tools(&strings(&[
        "--project=/repo",
        "--since=2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
        "--json",
    ]))
    .unwrap();
    match tools {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Tools(args)) => {
            assert_eq!(args.project.as_deref(), Some("/repo"));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let projects = parse_sessions_projects(&strings(&[
        "--tool=Write",
        "--since=2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
    ]))
    .unwrap();
    match projects {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Projects(args)) => {
            assert_eq!(args.tool.as_deref(), Some("Write"));
            assert_eq!(args.until.as_deref(), Some("2026-01-02T00:00:00+00:00"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let index = parse_sessions_index(&strings(&[
        "--path=/tmp/transcripts",
        "--since=2026-01-01T00:00:00Z",
        "--force",
    ]))
    .unwrap();
    match index {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Index(args)) => {
            assert_eq!(args.path.as_deref(), Some("/tmp/transcripts"));
            assert_eq!(args.since.as_deref(), Some("2026-01-01T00:00:00+00:00"));
            assert!(args.force);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let add = parse_sessions_add(&strings(&[
        "--file=/tmp/session.jsonl",
        "--force",
        "--json",
    ]))
    .unwrap();
    match add {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Add(args)) => {
            assert_eq!(args.file, "/tmp/session.jsonl");
            assert!(args.force);
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_sessions_watch_checkpoint_error_and_doctor_commands_accept_flags() {
    let watch = parse_sessions_watch(&strings(&[
        "--path=/tmp/sessions",
        "--debounce-ms=25",
        "--settle-ms=30",
        "--max-retries=2",
        "--no-initial-scan",
        "--json",
    ]))
    .unwrap();
    match watch {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Watch(args)) => {
            assert_eq!(args.path.as_deref(), Some("/tmp/sessions"));
            assert_eq!(args.debounce_ms, 25);
            assert_eq!(args.settle_ms, 30);
            assert_eq!(args.max_retries, 2);
            assert!(args.no_initial_scan);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let checkpoints =
        parse_sessions_checkpoints(&strings(&["--errors", "--missing", "--limit=11"])).unwrap();
    match checkpoints {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Checkpoints(args)) => {
            assert!(args.errors_only);
            assert!(args.missing_only);
            assert_eq!(args.limit, Some(11));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let errors = parse_sessions_errors(&strings(&["--limit=6", "--json"])).unwrap();
    match errors {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Errors(args)) => {
            assert_eq!(args.limit, Some(6));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let prune =
        parse_sessions_prune_checkpoints(&strings(&["--missing", "--dry-run", "--limit=2"]))
            .unwrap();
    match prune {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::PruneCheckpoints(args)) => {
            assert!(args.missing_only);
            assert!(args.dry_run);
            assert_eq!(args.limit, Some(2));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let doctor = parse_sessions_doctor(&strings(&["--strict-permissions", "--json"])).unwrap();
    match doctor {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Doctor(args)) => {
            assert!(args.strict_permissions);
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_sessions_reports_errors_for_missing_required_or_unexpected_args() {
    for (subcommand, parser, args, expected) in [
        (
            "context",
            parse_sessions_context as fn(&[String]) -> anyhow::Result<crate::cli::CliCommand>,
            vec!["--tool=Edit"],
            "requires PROJECT",
        ),
        (
            "add",
            parse_sessions_add,
            vec!["--json"],
            "requires a file path",
        ),
    ] {
        let err = parser(&strings(&args)).unwrap_err().to_string();
        assert!(
            err.contains(expected),
            "{subcommand} expected {expected:?}, got {err:?}"
        );
    }

    assert!(parse_sessions_prune_checkpoints(&strings(&["--dry-run"])).is_ok());

    for (parser, args, expected) in [
        (
            parse_sessions_abuse as fn(&[String]) -> anyhow::Result<crate::cli::CliCommand>,
            vec!["extra"],
            "unexpected sessions abuse argument",
        ),
        (
            parse_sessions_correlate,
            vec!["extra"],
            "unexpected sessions correlate argument",
        ),
        (
            parse_sessions_context,
            vec!["/repo", "extra"],
            "unexpected sessions context argument",
        ),
    ] {
        let err = parser(&strings(&args)).unwrap_err().to_string();
        assert!(err.contains(expected), "got {err:?}");
    }
}

#[test]
fn parse_sessions_unknown_subcommand_suggests_close_match() {
    let err = parse_sessions_command(&strings(&["serach", "error"]))
        .unwrap_err()
        .to_string();

    assert!(err.contains("Did you mean `search`?"), "got: {err}");
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[test]
fn parses_sessions_skills_backfill_with_flags() {
    let command = parse_sessions_command(&strings(&[
        "skills",
        "backfill",
        "--since",
        "30d",
        "--limit",
        "10000",
        "--dry-run",
    ]))
    .unwrap();

    match command {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillsBackfill(args)) => {
            // --since is normalized to RFC3339 by norm_time, not stored raw.
            let since = args.since.expect("since should be set");
            assert!(since.ends_with("+00:00"), "--since not normalized: {since}");
            assert_eq!(args.limit, Some(10000));
            assert!(args.dry_run);
        }
        other => panic!("expected SessionsCommand::SkillsBackfill, got {other:?}"),
    }
}

#[test]
fn parses_sessions_skills_list_with_project_filter() {
    let command = parse_sessions_command(&strings(&[
        "skills",
        "--project",
        "cortex",
        "--limit",
        "20",
    ]))
    .unwrap();

    match command {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::Skills(args)) => {
            assert_eq!(args.project.as_deref(), Some("cortex"));
            assert_eq!(args.limit, Some(20));
        }
        other => panic!("expected SessionsCommand::Skills, got {other:?}"),
    }
}

#[test]
fn skill_investigate_binds_bare_positional_to_skill() {
    let cmd = parse_sessions_command(&strings(&["skillinvestigate", "lavra:lavra-plan"])).unwrap();
    match cmd {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.skill.as_deref(), Some("lavra:lavra-plan"));
            assert!(args.incident_id.is_none());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_accepts_since_and_tool_flags_with_positional() {
    let cmd = parse_sessions_command(&strings(&[
        "skillinvestigate",
        "lavra:lavra-plan",
        "--since",
        "7d",
        "--tool",
        "codex",
    ]))
    .unwrap();
    match cmd {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.skill.as_deref(), Some("lavra:lavra-plan"));
            assert_eq!(args.tool.as_deref(), Some("codex"));
            assert!(args.since.is_some());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_incident_id_flag_overrides_but_does_not_require_positional() {
    let cmd = parse_sessions_command(&strings(&[
        "skillinvestigate",
        "--incident-id",
        "skill-inc-deadbeef",
    ]))
    .unwrap();
    match cmd {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.incident_id.as_deref(), Some("skill-inc-deadbeef"));
            assert!(args.skill.is_none());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_plugin_flag_for_plugin_level_investigation() {
    let cmd = parse_sessions_command(&strings(&["skillinvestigate", "--plugin", "lavra"])).unwrap();
    match cmd {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillInvestigate(args)) => {
            assert_eq!(args.plugin.as_deref(), Some("lavra"));
            assert!(args.skill.is_none());
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_investigate_all_and_limit_flags() {
    let cmd = parse_sessions_command(&strings(&[
        "skillinvestigate",
        "lavra:lavra-plan",
        "--all",
        "--limit",
        "5",
    ]))
    .unwrap();
    match cmd {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillInvestigate(args)) => {
            assert!(args.all);
            assert_eq!(args.limit, Some(5));
        }
        other => panic!("expected SkillInvestigate, got {other:?}"),
    }
}

#[test]
fn skill_incidents_accepts_skill_and_min_score_flags() {
    let cmd = parse_sessions_command(&strings(&[
        "skillincidents",
        "--skill",
        "lavra:lavra-plan",
        "--min-score",
        "35",
    ]))
    .unwrap();
    match cmd {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillIncidents(args)) => {
            assert_eq!(args.skill.as_deref(), Some("lavra:lavra-plan"));
            assert_eq!(args.min_score.as_deref(), Some("35"));
        }
        other => panic!("expected SkillIncidents, got {other:?}"),
    }
}

#[test]
fn skill_incidents_accepts_repeated_signal_flags() {
    let cmd = parse_sessions_command(&strings(&[
        "skillincidents",
        "--signal",
        "tool_failure_after_skill",
        "--signal",
        "user_correction_after_skill",
    ]))
    .unwrap();
    match cmd {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::SkillIncidents(args)) => {
            assert_eq!(
                args.signals,
                vec![
                    "tool_failure_after_skill".to_string(),
                    "user_correction_after_skill".to_string()
                ]
            );
        }
        other => panic!("expected SkillIncidents, got {other:?}"),
    }
}

#[test]
fn hook_commands_bind_bare_hook_names() {
    let incidents = parse_sessions_command(&strings(&["hookincidents", "format-on-save"]))
        .expect("hook incident list parses");
    match incidents {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::HookIncidents(args)) => {
            assert_eq!(args.hook_name.as_deref(), Some("format-on-save"));
        }
        other => panic!("expected HookIncidents, got {other:?}"),
    }

    let investigate = parse_sessions_command(&strings(&["hookinvestigate", "format-on-save"]))
        .expect("hook investigation parses");
    match investigate {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::HookInvestigate(args)) => {
            assert_eq!(args.hook_name.as_deref(), Some("format-on-save"));
        }
        other => panic!("expected HookInvestigate, got {other:?}"),
    }
}
