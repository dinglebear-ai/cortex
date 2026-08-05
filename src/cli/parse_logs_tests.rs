use super::*;

#[test]
fn parse_timeline_collects_bucket_group_and_filters() {
    let args = strings(&[
        "--bucket",
        "hour",
        "--group-by",
        "hostname",
        "--host=host1",
        "--json",
    ]);

    let command = parse_timeline(&args).unwrap();

    match command {
        crate::cli::CliCommand::Timeline(args) => {
            assert_eq!(args.bucket.as_deref(), Some("hour"));
            assert_eq!(args.group_by.as_deref(), Some("hostname"));
            assert_eq!(args.host.as_deref(), Some("host1"));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_hosts_sources_accepts_limit_and_offset() {
    let args = strings(&["--limit", "10", "--offset=5"]);

    let command = parse_hosts_sources(&args).unwrap();

    match command {
        crate::cli::CliCommand::Hosts(crate::cli::HostsCommand::Sources(args)) => {
            assert_eq!(args.limit, Some(10));
            assert_eq!(args.offset, Some(5));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_errors_accepts_limit_for_bounded_agent_output() {
    let args = strings(&["--since=2026-01-01T00:00:00Z", "--limit", "10", "--json"]);

    let command = parse_errors(&args).unwrap();

    match command {
        crate::cli::CliCommand::Errors(args) => {
            assert_eq!(args.since.as_deref(), Some("2026-01-01T00:00:00+00:00"));
            assert_eq!(args.limit, Some(10));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_filter_collects_structured_filters_and_rejects_query_terms() {
    let args = strings(&[
        "--source-kind=docker-stream",
        "--docker-host",
        "devhost",
        "--container=cortex",
        "--stream=stdout",
        "--event-action",
        "die",
        "--tool=claude",
        "--project=/home/jmagar/workspace/cortex",
        "--session-id=abc123",
        "--limit=25",
        "--json",
    ]);

    let command = parse_filter(&args).unwrap();

    match command {
        crate::cli::CliCommand::Filter(args) => {
            assert_eq!(args.source_kind.as_deref(), Some("docker-stream"));
            assert_eq!(args.docker_host.as_deref(), Some("devhost"));
            assert_eq!(args.container.as_deref(), Some("cortex"));
            assert_eq!(args.stream.as_deref(), Some("stdout"));
            assert_eq!(args.event_action.as_deref(), Some("die"));
            assert_eq!(args.tool.as_deref(), Some("claude"));
            assert_eq!(
                args.project.as_deref(),
                Some("/home/jmagar/workspace/cortex")
            );
            assert_eq!(args.session_id.as_deref(), Some("abc123"));
            assert_eq!(args.limit, Some(25));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let err = parse_filter(&strings(&["error"])).unwrap_err();
    assert!(
        err.to_string()
            .contains("filter does not accept positional query terms")
    );
}

#[test]
fn parse_search_tail_sessions_incident_and_correlate_cover_common_filters() {
    let search = parse_search(&strings(&[
        "--host=host1",
        "--source=10.0.0.1",
        "--severity=err",
        "--app=cortex",
        "--facility=daemon",
        "--exclude-facility=kern",
        "--since=2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
        "--received-since=2026-01-03T00:00:00Z",
        "--received-until=2026-01-04T00:00:00Z",
        "--limit=30",
        "--json",
        "disk",
        "full",
    ]))
    .unwrap();
    match search {
        crate::cli::CliCommand::Search(args) => {
            assert_eq!(args.query.as_deref(), Some("disk full"));
            assert_eq!(args.host.as_deref(), Some("host1"));
            assert_eq!(
                args.received_until.as_deref(),
                Some("2026-01-04T00:00:00+00:00")
            );
            assert_eq!(args.limit, Some(30));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let tail = parse_tail(&strings(&["--host=host1", "--source=10.0.0.1", "-n", "12"])).unwrap();
    match tail {
        crate::cli::CliCommand::Tail(args) => {
            assert_eq!(args.host.as_deref(), Some("host1"));
            assert_eq!(args.n, Some(12));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let sessions = parse_sessions(&strings(&[
        "--project=/repo",
        "--tool=Bash",
        "--host=host1",
        "--since=2026-01-01T00:00:00Z",
        "--until=2026-01-02T00:00:00Z",
        "--limit=4",
    ]))
    .unwrap();
    match sessions {
        crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::List(args)) => {
            assert_eq!(args.project.as_deref(), Some("/repo"));
            assert_eq!(args.tool.as_deref(), Some("Bash"));
            assert_eq!(args.limit, Some(4));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let incident = parse_incident(&strings(&[
        "--around=2026-01-01T00:00:00Z",
        "--minutes=10",
        "--host=host1",
        "--limit=50",
    ]))
    .unwrap();
    match incident {
        crate::cli::CliCommand::Incident(args) => {
            // The time value is normalized to RFC3339 at parse time.
            assert_eq!(args.around, "2026-01-01T00:00:00+00:00");
            assert_eq!(args.minutes, Some(10));
            assert_eq!(args.host.as_deref(), Some("host1"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let correlate = parse_correlate(&strings(&[
        "--reference-time=2026-01-01T00:00:00Z",
        "--window-minutes=5",
        "--severity-min=warn",
        "--host=host1",
        "--source=10.0.0.1",
        "--query=panic",
        "--limit=99",
    ]))
    .unwrap();
    match correlate {
        crate::cli::CliCommand::Correlate(args) => {
            // The time value is normalized to RFC3339 at parse time.
            assert_eq!(
                args.reference_time.as_deref(),
                Some("2026-01-01T00:00:00+00:00")
            );
            assert_eq!(args.window_minutes, Some(5));
            assert_eq!(args.query.as_deref(), Some("panic"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_correlate_accepts_query_alone_without_reference_time() {
    let correlate = parse_correlate(&strings(&["--query=qbittorrent keeps dying"])).unwrap();
    match correlate {
        crate::cli::CliCommand::Correlate(args) => {
            assert_eq!(args.reference_time, None);
            assert_eq!(args.query.as_deref(), Some("qbittorrent keeps dying"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_log_commands_report_help_and_unknown_argument_errors() {
    for (parser, args, expected) in [
        (
            parse_search as fn(&[String]) -> anyhow::Result<crate::cli::CliCommand>,
            vec!["--help"],
            "use `cortex --help`",
        ),
        (parse_filter, vec!["--bogus"], "unknown filter option"),
        (parse_tail, vec!["--bogus"], "unknown tail option"),
        (
            parse_sessions,
            vec!["extra"],
            "unexpected sessions argument",
        ),
        (
            parse_hosts_sources,
            vec!["--bogus"],
            "unknown hosts sources option",
        ),
        (parse_timeline, vec!["--bogus"], "unknown timeline option"),
        (parse_patterns, vec!["--bogus"], "unknown patterns option"),
    ] {
        let err = parser(&strings(&args)).unwrap_err().to_string();
        assert!(err.contains(expected), "expected {expected:?}, got {err:?}");
    }

    let err = parse_ingest_rate_args(&strings(&["--host"]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown ingestrate option"), "got {err:?}");
}

#[test]
fn parse_patterns_accepts_limit_alias_for_top_n() {
    let args = strings(&["--limit=7"]);

    let command = parse_patterns(&args).unwrap();

    match command {
        crate::cli::CliCommand::Patterns(args) => {
            assert_eq!(args.top_n, Some(7));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[test]
fn search_normalizes_relative_from() {
    let cmd = parse_search(&strings(&["error", "--since", "1h"])).unwrap();
    let crate::cli::CliCommand::Search(args) = cmd else {
        panic!("expected Search");
    };
    let from = args.since.expect("from set");
    // Relative input is normalized to an absolute RFC3339 timestamp at parse time.
    assert!(
        from.contains('T') && from.ends_with("+00:00"),
        "expected normalized RFC3339, got {from}"
    );
}

#[test]
fn search_grep_sets_literal_and_rejects_with_query() {
    let cmd = parse_search(&strings(&["--grep", "smoke-test"])).unwrap();
    let crate::cli::CliCommand::Search(args) = cmd else {
        panic!("expected Search");
    };
    assert_eq!(args.grep.as_deref(), Some("smoke-test"));

    // --grep together with a positional query is an error.
    let err = parse_search(&strings(&["error", "--grep", "x"]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("--grep"), "should explain the conflict: {err}");
}

#[test]
fn search_grep_equals_form_and_rejects_empty() {
    let cmd = parse_search(&strings(&["--grep=smoke-test"])).unwrap();
    let crate::cli::CliCommand::Search(args) = cmd else {
        panic!("expected Search");
    };
    assert_eq!(args.grep.as_deref(), Some("smoke-test"));
    // Whitespace-only --grep is rejected rather than matching nothing silently.
    assert!(parse_search(&strings(&["--grep", "   "])).is_err());
}

#[test]
fn filter_and_sessions_normalize_relative_from() {
    let filter = parse_filter(&strings(&["--since", "2d"])).unwrap();
    let crate::cli::CliCommand::Filter(args) = filter else {
        panic!("expected Filter");
    };
    assert!(
        args.since.as_deref().unwrap().ends_with("+00:00"),
        "filter --since should normalize: {:?}",
        args.since
    );

    // Equals form is normalized too.
    let sessions = parse_sessions(&strings(&["--since=1h"])).unwrap();
    let crate::cli::CliCommand::Sessions(crate::cli::SessionsCommand::List(args)) = sessions else {
        panic!("expected Sessions");
    };
    assert!(
        args.since.as_deref().unwrap().ends_with("+00:00"),
        "sessions --since= should normalize: {:?}",
        args.since
    );
}

#[test]
fn tail_positional_sets_host_and_default_limit() {
    let cmd = parse_tail(&strings(&["devhost"])).unwrap();
    let crate::cli::CliCommand::Tail(args) = cmd else {
        panic!("expected Tail")
    };
    assert_eq!(args.host.as_deref(), Some("devhost"));
    assert_eq!(args.n, None); // shared service applies the default
}

#[test]
fn tail_explicit_limit_overrides_default() {
    let cmd = parse_tail(&strings(&["devhost", "-n", "10"])).unwrap();
    let crate::cli::CliCommand::Tail(args) = cmd else {
        panic!("expected Tail")
    };
    assert_eq!(args.host.as_deref(), Some("devhost"));
    assert_eq!(args.n, Some(10));
}

#[test]
fn tail_rejects_two_positionals() {
    let err = parse_tail(&strings(&["devhost", "nashost"]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("at most one"), "{err}");
}

#[test]
fn tail_accepts_limit_flag_for_count() {
    // `--limit` is a documented alias for `-n`; both set the row count.
    for cmd in [
        parse_tail(&strings(&["--limit", "7"])).unwrap(),
        parse_tail(&strings(&["--limit=7"])).unwrap(),
    ] {
        let crate::cli::CliCommand::Tail(args) = cmd else {
            panic!("expected Tail")
        };
        assert_eq!(args.n, Some(7));
    }
}

#[test]
fn tail_positional_and_host_flag_are_mutually_exclusive() {
    let err = parse_tail(&strings(&["bar", "--host", "foo"]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn search_applies_default_limit() {
    let cmd = parse_search(&strings(&["oom"])).unwrap();
    let crate::cli::CliCommand::Search(args) = cmd else {
        panic!("expected Search")
    };
    assert_eq!(args.query.as_deref(), Some("oom"));
    assert_eq!(args.limit, None); // shared service applies the default
}

#[test]
fn search_explicit_limit_overrides_default() {
    let cmd = parse_search(&strings(&["oom", "--limit", "5"])).unwrap();
    let crate::cli::CliCommand::Search(args) = cmd else {
        panic!("expected Search")
    };
    assert_eq!(args.limit, Some(5));
}

#[test]
fn errors_defaults_to_one_hour_window() {
    let cmd = parse_errors(&strings(&[])).unwrap();
    let crate::cli::CliCommand::Errors(args) = cmd else {
        panic!("expected Errors")
    };
    assert!(args.since.is_none()); // shared service applies the one-hour default
}

#[test]
fn errors_explicit_since_overrides_default() {
    let cmd = parse_errors(&strings(&["--since", "2026-01-01T00:00:00Z"])).unwrap();
    let crate::cli::CliCommand::Errors(args) = cmd else {
        panic!("expected Errors")
    };
    assert_eq!(args.since.as_deref(), Some("2026-01-01T00:00:00+00:00"));
}

#[test]
fn timeline_patterns_incident_correlate_normalize_relative_time() {
    // timeline --since/--until accept relative values like the other time flags.
    let timeline = parse_timeline(&strings(&["--since", "2d", "--until=1h"])).unwrap();
    let crate::cli::CliCommand::Timeline(args) = timeline else {
        panic!("expected Timeline");
    };
    assert!(
        args.since.as_deref().unwrap().ends_with("+00:00"),
        "timeline --since should normalize: {:?}",
        args.since
    );
    assert!(
        args.until.as_deref().unwrap().ends_with("+00:00"),
        "timeline --until= should normalize: {:?}",
        args.until
    );

    let patterns = parse_patterns(&strings(&["--since=yesterday"])).unwrap();
    let crate::cli::CliCommand::Patterns(args) = patterns else {
        panic!("expected Patterns");
    };
    assert!(
        args.since.as_deref().unwrap().ends_with("+00:00"),
        "patterns --since= should normalize: {:?}",
        args.since
    );

    // incident --around and correlate --reference-time normalize and reject garbage.
    let incident = parse_incident(&strings(&["--around", "1h"])).unwrap();
    let crate::cli::CliCommand::Incident(args) = incident else {
        panic!("expected Incident");
    };
    assert!(
        args.around.ends_with("+00:00"),
        "incident --around should normalize: {:?}",
        args.around
    );

    let correlate = parse_correlate(&strings(&["--reference-time=1h"])).unwrap();
    let crate::cli::CliCommand::Correlate(args) = correlate else {
        panic!("expected Correlate");
    };
    assert!(
        args.reference_time.as_deref().unwrap().ends_with("+00:00"),
        "correlate --reference-time= should normalize: {:?}",
        args.reference_time
    );

    // Correlate also accepts a positional reference time, normalized identically.
    let correlate_pos = parse_correlate(&strings(&["2d"])).unwrap();
    let crate::cli::CliCommand::Correlate(args) = correlate_pos else {
        panic!("expected Correlate");
    };
    assert!(
        args.reference_time.as_deref().unwrap().ends_with("+00:00"),
        "correlate positional time should normalize: {:?}",
        args.reference_time
    );

    // A non-time value is now rejected at parse time instead of passing through.
    assert!(parse_incident(&strings(&["--around=not-a-time"])).is_err());
    assert!(parse_correlate(&strings(&["--reference-time=not-a-time"])).is_err());
}
