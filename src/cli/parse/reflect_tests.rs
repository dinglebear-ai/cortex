use super::*;

fn parse(args: &[&str]) -> anyhow::Result<ReflectArgs> {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    match parse_reflect(&args)? {
        CliCommand::Reflect(parsed) => Ok(parsed),
        other => panic!("expected reflect, got {other:?}"),
    }
}

#[test]
fn defaults_are_llm_on_and_seven_days() {
    let parsed = parse(&["skills"]).unwrap();
    assert_eq!(parsed.kinds, vec![ReflectKind::Skill]);
    assert!(!parsed.no_llm && !parsed.no_index && !parsed.json);
    assert_eq!(parsed.max_assess, DEFAULT_REFLECT_MAX_ASSESS);
    assert_eq!(parsed.db, None);
    assert!(
        !parsed.since.is_empty(),
        "default --since 7d must be normalized"
    );
}

#[test]
fn each_kind_is_one_positional() {
    assert_eq!(parse(&["skills"]).unwrap().kinds, vec![ReflectKind::Skill]);
    assert_eq!(parse(&["mcp"]).unwrap().kinds, vec![ReflectKind::Mcp]);
    assert_eq!(parse(&["hooks"]).unwrap().kinds, vec![ReflectKind::Hook]);
    // Singular forms are accepted too.
    assert_eq!(parse(&["skill"]).unwrap().kinds, vec![ReflectKind::Skill]);
    assert_eq!(parse(&["hook"]).unwrap().kinds, vec![ReflectKind::Hook]);
}

#[test]
fn every_flag_is_parsed() {
    let parsed = parse(&[
        "--since",
        "2026-09-01T00:00:00Z",
        "hooks",
        "--until",
        "2026-09-02T00:00:00Z",
        "--project",
        "/p",
        "--tool",
        "codex",
        "--no-llm",
        "--max-assess",
        "0",
        "--no-index",
        "--db",
        "/tmp/r.db",
        "--json",
    ])
    .unwrap();
    assert_eq!(parsed.kinds, vec![ReflectKind::Hook]);
    assert!(parsed.since.starts_with("2026-09-01"));
    assert!(parsed.until.as_deref().unwrap().starts_with("2026-09-02"));
    assert_eq!(parsed.project.as_deref(), Some("/p"));
    assert_eq!(parsed.tool.as_deref(), Some("codex"));
    assert!(parsed.no_llm && parsed.no_index && parsed.json);
    assert_eq!(parsed.max_assess, 0);
    assert_eq!(parsed.db, Some(std::path::PathBuf::from("/tmp/r.db")));
}

#[test]
fn a_kind_is_required() {
    let error = parse(&[]).unwrap_err().to_string();
    assert!(error.contains("cortex reflect skills"), "{error}");
}

#[test]
fn unknown_and_extra_kinds_are_rejected() {
    let error = parse(&["abuse"]).unwrap_err().to_string();
    assert!(error.contains("unknown kind 'abuse'"), "{error}");
    let error = parse(&["skills", "mcp"]).unwrap_err().to_string();
    assert!(error.contains("one kind"), "{error}");
}

#[test]
fn removed_and_unknown_options_are_rejected() {
    assert!(parse(&["skills", "--kinds", "skill"]).is_err());
    assert!(parse(&["skills", "--out", "x.md"]).is_err());
    assert!(parse(&["skills", "--all"]).is_err());
}
