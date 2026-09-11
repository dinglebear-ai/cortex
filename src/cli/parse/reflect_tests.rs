use super::*;

fn parse(args: &[&str]) -> anyhow::Result<ReflectArgs> {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    match parse_reflect(&args)? {
        CliCommand::Reflect(parsed) => Ok(parsed),
        other => panic!("expected reflect, got {other:?}"),
    }
}

#[test]
fn defaults_cover_all_kinds_llm_on_and_seven_days() {
    let parsed = parse(&[]).unwrap();
    assert_eq!(parsed.kinds, ReflectKind::ALL.to_vec());
    assert!(!parsed.no_llm && !parsed.no_index && !parsed.json);
    assert_eq!(parsed.max_assess, DEFAULT_REFLECT_MAX_ASSESS);
    assert_eq!(parsed.db, None);
    assert!(
        !parsed.since.is_empty(),
        "default --since 7d must be normalized"
    );
}

#[test]
fn every_flag_is_parsed() {
    let parsed = parse(&[
        "--since",
        "2026-09-01T00:00:00Z",
        "--until",
        "2026-09-02T00:00:00Z",
        "--project",
        "/p",
        "--tool",
        "codex",
        "--kinds",
        "hook,skill",
        "--no-llm",
        "--max-assess",
        "0",
        "--no-index",
        "--db",
        "/tmp/r.db",
        "--json",
    ])
    .unwrap();
    assert!(parsed.since.starts_with("2026-09-01"));
    assert!(parsed.until.as_deref().unwrap().starts_with("2026-09-02"));
    assert_eq!(parsed.project.as_deref(), Some("/p"));
    assert_eq!(parsed.tool.as_deref(), Some("codex"));
    assert_eq!(parsed.kinds, vec![ReflectKind::Hook, ReflectKind::Skill]);
    assert!(parsed.no_llm && parsed.no_index && parsed.json);
    assert_eq!(parsed.max_assess, 0);
    assert_eq!(parsed.db, Some(std::path::PathBuf::from("/tmp/r.db")));
}

#[test]
fn kinds_are_deduplicated() {
    assert_eq!(
        parse(&["--kinds", "mcp,mcp, mcp"]).unwrap().kinds,
        vec![ReflectKind::Mcp]
    );
}

#[test]
fn unknown_kind_is_rejected() {
    let error = parse(&["--kinds", "skill,abuse"]).unwrap_err().to_string();
    assert!(error.contains("unknown kind 'abuse'"), "{error}");
}

#[test]
fn empty_kinds_is_rejected() {
    assert!(parse(&["--kinds", ","]).is_err());
}

#[test]
fn removed_and_unknown_options_are_rejected() {
    assert!(parse(&["--out", "x.md"]).is_err());
    assert!(parse(&["--all"]).is_err());
}
