use super::*;

/// Every top-level token the parser accepts (mirrors the `match` in
/// `src/cli/parse.rs` plus the Mode-level commands in `src/main.rs`). If a
/// command is added to the parser, add it here and to `CATALOG`.
const PARSER_TOKENS: &[&str] = &[
    // CliCommand::parse (src/cli/parse.rs)
    "search",
    "filter",
    "tail",
    "hosts",
    "sessions",
    "assess",
    "analysis",
    "state",
    "ingest",
    "alerts",
    "heartbeat",
    "correlate",
    "stats",
    "compose",
    "setup",
    "db",
    "config",
    "entity",
    "graph",
    "timeline",
    "apps",
    "artifactevents",
    "completions",
    // Mode-level (src/main.rs)
    "serve",
    "mcp",
    "doctor",
    "update",
];

#[test]
fn catalog_covers_every_parser_token() {
    for token in PARSER_TOKENS {
        assert!(
            is_known(token),
            "parser accepts `{token}` but it has no CATALOG entry — add it to help.rs"
        );
    }
}

#[test]
fn every_catalog_entry_is_in_exactly_one_section() {
    for doc in CATALOG {
        let count = SECTIONS
            .iter()
            .filter(|(_, names)| names.contains(&doc.name))
            .count();
        assert_eq!(
            count, 1,
            "`{}` must appear in exactly one section, found {count}",
            doc.name
        );
    }
}

#[test]
fn documented_command_tokens_are_single_words() {
    for doc in CATALOG {
        assert!(
            !doc.name.contains('-'),
            "hyphenated command root: {}",
            doc.name
        );
    }
    for doc in NESTED_CATALOG {
        assert!(
            doc.path
                .split_whitespace()
                .all(|token| !token.contains('-')),
            "hyphenated command path: {}",
            doc.path
        );
    }
}

#[test]
fn every_section_name_is_a_real_command() {
    for (section, names) in SECTIONS {
        for name in *names {
            assert!(
                is_known(name),
                "section `{section}` lists `{name}` which is not in CATALOG"
            );
        }
    }
}

#[test]
fn top_level_help_plain_lists_sections_and_commands() {
    let out = render_top_level(false);
    assert!(!out.contains('\x1b'), "color=false must emit no ANSI");
    assert!(out.contains("CORTEX CLI"));
    assert!(out.contains("Quick Start"));
    assert!(out.contains("Commands"));
    assert!(out.contains("Search & Logs"));
    assert!(out.contains("hosts"));
    assert!(out.contains("ingest"));
    assert!(out.contains("→ Run cortex <command> --help"));
}

#[test]
fn top_level_help_colored_emits_cyan_and_white() {
    let out = render_top_level(true);
    assert!(out.contains('\x1b'));
    assert!(out.contains("38;2;41;182;246"), "cyan headers");
    assert!(out.contains("38;2;230;244;251"), "white command names");
}

#[test]
fn command_help_shows_detailed_flags() {
    let out = render_command("db", false).expect("db is known");
    assert!(out.contains("--check-coord"), "db status flags present");
    assert!(out.contains("cortex db vacuum"));
    assert!(render_command("definitely-not-a-command", false).is_none());
}

#[test]
fn nested_help_shows_subcommand_specific_usage() {
    let out = render_command("sessions search", false).expect("sessions search is known");
    assert!(out.contains("cortex sessions search QUERY"), "got: {out}");
    assert!(!out.contains("cortex sessions investigate"), "got: {out}");

    let out = render_command("sessions investigate", false).expect("sessions investigate is known");
    assert!(out.contains("--detail compact|full"), "got: {out}");
    assert!(out.contains("--include-transcript"), "got: {out}");

    let out = render_command("ingest inventory", false).expect("ingest inventory is known");
    assert!(
        out.contains("cortex ingest inventory refresh [--json]"),
        "got: {out}"
    );
    assert!(
        out.contains("cortex ingest inventory status [--json]"),
        "got: {out}"
    );

    let out = render_command("ingest filetail", false).expect("ingest filetail is known");
    assert!(
        out.contains("cortex ingest filetail add PATH"),
        "got: {out}"
    );
    assert!(out.contains("--from-start"), "got: {out}");

    assert!(render_command("ai search", false).is_none());
    assert!(render_command("inventory refresh", false).is_none());
    assert!(render_command("file-tail add", false).is_none());
    assert!(render_command("sig ack", false).is_none());
    assert!(render_command("notify recent", false).is_none());
}

#[test]
fn setup_doctor_has_nested_help() {
    // `cortex setup doctor --help` must show doctor-specific help, not fall back
    // to the generic setup help (it is advertised in the setup CATALOG entry).
    let out = render_command("setup doctor", false).expect("setup doctor is known");
    assert!(out.contains("cortex setup doctor"), "got: {out}");
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        classify_help(&v(&["setup", "doctor", "--help"])),
        HelpRequest::Command("setup doctor".to_string())
    );
}

#[test]
fn classify_help_skips_global_option_values() {
    // A value-bearing global option's value must not be mistaken for the command
    // path: `cortex --server URL db status --help` resolves to `db status`.
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        classify_help(&v(&[
            "--server",
            "http://127.0.0.1:3100",
            "db",
            "status",
            "--help"
        ])),
        HelpRequest::Command("db status".to_string())
    );
    assert_eq!(
        classify_help(&v(&["--token", "secret", "search", "--help"])),
        HelpRequest::Command("search".to_string())
    );
}

#[test]
fn classify_help_distinguishes_top_level_command_and_none() {
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    assert_eq!(classify_help(&v(&["--help"])), HelpRequest::TopLevel);
    assert_eq!(classify_help(&v(&["help"])), HelpRequest::TopLevel);
    assert_eq!(classify_help(&v(&["-h"])), HelpRequest::TopLevel);
    assert_eq!(
        classify_help(&v(&["db", "status", "--help"])),
        HelpRequest::Command("db status".to_string())
    );
    assert_eq!(
        classify_help(&v(&["sessions", "search", "--help"])),
        HelpRequest::Command("sessions search".to_string())
    );
    assert_eq!(
        classify_help(&v(&["search", "--help"])),
        HelpRequest::Command("search".to_string())
    );
    // Unknown command with a help flag falls back to the top-level banner.
    assert_eq!(
        classify_help(&v(&["bogus", "--help"])),
        HelpRequest::TopLevel
    );
    // No help token → not a help request.
    assert_eq!(classify_help(&v(&["search", "foo"])), HelpRequest::None);
    assert_eq!(classify_help(&v(&[])), HelpRequest::None);

    // Regression: the bare word `help` as a free-text QUERY (not in command
    // position) must NOT trigger help — these run the actual search.
    assert_eq!(classify_help(&v(&["search", "help"])), HelpRequest::None);
    assert_eq!(
        classify_help(&v(&["sessions", "search", "help"])),
        HelpRequest::None
    );
    assert_eq!(
        classify_help(&v(&["sessions", "abuse", "--term", "help"])),
        HelpRequest::None
    );
    // But `help` in command position is still a top-level request.
    assert_eq!(classify_help(&v(&["help"])), HelpRequest::TopLevel);
    assert_eq!(
        classify_help(&v(&["help", "search"])),
        HelpRequest::Command("search".to_string())
    );
    // `--` sentinel hides a wrapped command's --help.
    assert_eq!(
        classify_help(&v(&[
            "ingest",
            "agent-command",
            "wrap",
            "--spool",
            "/x",
            "--",
            "tool",
            "--help"
        ])),
        HelpRequest::None
    );
}

#[test]
fn command_help_includes_registry_examples() {
    let body = render_command("search", false).expect("search help renders");
    assert!(
        body.contains("Examples"),
        "search help should have an Examples block"
    );
    assert!(
        body.contains("cortex search"),
        "search help should include a registry example: {body}"
    );
}
