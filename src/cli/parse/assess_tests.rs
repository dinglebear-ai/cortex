use super::*;
use crate::cli::args::{AssessCommand, CliCommand};

#[test]
fn parse_assess_requires_subcommand() {
    let err = parse_assess(&[]).unwrap_err();
    assert!(format!("{err}").contains("assess requires a subcommand"));
}

#[test]
fn parse_assess_mcp_parses_positional_target() {
    let cmd = parse_assess(&["mcp".to_string(), "labby".to_string()]).unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Mcp(args)) => {
            assert_eq!(args.target.as_deref(), Some("labby"));
            assert_eq!(args.server, None);
            assert_eq!(args.tool_name, None);
            assert!(
                !args.no_llm,
                "LLM assessment must run by default (mirrors `cortex assess skill`)"
            );
            assert!(!args.all);
            assert_eq!(args.limit, None);
        }
        other => panic!("expected AssessCommand::Mcp, got {other:?}"),
    }
}

#[test]
fn parse_assess_mcp_accepts_server_and_tool_name_only() {
    let cmd = parse_assess(&[
        "mcp".to_string(),
        "--server".to_string(),
        "labby".to_string(),
        "--tool-name".to_string(),
        "search".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Mcp(args)) => {
            assert_eq!(args.target, None);
            assert_eq!(args.server.as_deref(), Some("labby"));
            assert_eq!(args.tool_name.as_deref(), Some("search"));
        }
        other => panic!("expected AssessCommand::Mcp, got {other:?}"),
    }
}

#[test]
fn parse_assess_mcp_rejects_missing_target_server_and_tool_name() {
    let err = parse_assess(&["mcp".to_string()]).unwrap_err();
    assert!(format!("{err}").contains("an mcp server/tool name is required"));
}

#[test]
fn parse_assess_hooks_parses_with_no_args() {
    let cmd = parse_assess(&["hooks".to_string()]).unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Hooks(args)) => {
            assert_eq!(args.hook_name, None);
            assert!(
                !args.no_llm,
                "LLM assessment must run by default (mirrors `cortex assess skill`)"
            );
            assert!(!args.all);
            assert_eq!(args.limit, None);
        }
        other => panic!("expected AssessCommand::Hooks, got {other:?}"),
    }
}

#[test]
fn parse_assess_hooks_parses_positional_hook_name() {
    let cmd = parse_assess(&["hooks".to_string(), "format-on-save".to_string()]).unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Hooks(args)) => {
            assert_eq!(args.hook_name.as_deref(), Some("format-on-save"));
        }
        other => panic!("expected AssessCommand::Hooks, got {other:?}"),
    }
}

#[test]
fn parse_assess_hooks_flag_overrides_positional() {
    let cmd = parse_assess(&[
        "hooks".to_string(),
        "--hook".to_string(),
        "flag-hook".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Hooks(args)) => {
            assert_eq!(args.hook_name.as_deref(), Some("flag-hook"));
        }
        other => panic!("expected AssessCommand::Hooks, got {other:?}"),
    }
}

#[test]
fn parse_assess_skill_parses_positional_skill_name() {
    let cmd = parse_assess(&["skill".to_string(), "frustration-assessment".to_string()]).unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Skill(args)) => {
            assert_eq!(args.skill.as_deref(), Some("frustration-assessment"));
            assert_eq!(args.plugin, None);
            assert!(
                !args.no_llm,
                "LLM assessment must run by default (mirrors `cortex sessions assess`)"
            );
            assert!(!args.all);
            assert_eq!(args.limit, None);
        }
        other => panic!("expected AssessCommand::Skill, got {other:?}"),
    }
}

#[test]
fn parse_assess_skill_accepts_plugin_only() {
    let cmd = parse_assess(&[
        "skill".to_string(),
        "--plugin".to_string(),
        "lavra".to_string(),
    ])
    .unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Skill(args)) => {
            assert_eq!(args.skill, None);
            assert_eq!(args.plugin.as_deref(), Some("lavra"));
        }
        other => panic!("expected AssessCommand::Skill, got {other:?}"),
    }
}

#[test]
fn parse_assess_skill_rejects_missing_positional_and_plugin() {
    let err = parse_assess(&["skill".to_string()]).unwrap_err();
    assert!(format!("{err}").contains("skill name, --plugin, or --incident-id is required"));
}

#[test]
fn parse_assess_abuse_defaults_to_no_incident_id() {
    let cmd = parse_assess(&["abuse".to_string()]).unwrap();
    match cmd {
        CliCommand::Assess(AssessCommand::Abuse(args)) => {
            assert_eq!(args.incident_id, None);
            assert!(!args.no_llm);
        }
        other => panic!("expected AssessCommand::Abuse, got {other:?}"),
    }
}

#[test]
fn parse_assess_unknown_subcommand_suggests() {
    let err = parse_assess(&["bogus".to_string()]).unwrap_err();
    assert!(format!("{err}").contains("bogus"));
}

#[test]
fn assess_skill_accepts_incident_id_without_skill_name() {
    let args = vec!["--incident-id".to_string(), "abc123".to_string()];
    let CliCommand::Assess(AssessCommand::Skill(parsed)) = parse_assess_skill_from(&args).unwrap()
    else {
        panic!("expected assess skill");
    };
    assert_eq!(parsed.incident_id.as_deref(), Some("abc123"));
    assert_eq!(parsed.skill, None);
}

#[test]
fn assess_mcp_accepts_incident_id_without_target() {
    let args = vec!["--incident-id".to_string(), "abc123".to_string()];
    let CliCommand::Assess(AssessCommand::Mcp(parsed)) = parse_assess_mcp_from(&args).unwrap()
    else {
        panic!("expected assess mcp");
    };
    assert_eq!(parsed.incident_id.as_deref(), Some("abc123"));
}

#[test]
fn assess_hooks_accepts_incident_id() {
    let args = vec!["--incident-id".to_string(), "abc123".to_string()];
    let CliCommand::Assess(AssessCommand::Hooks(parsed)) = parse_assess_hooks(&args).unwrap()
    else {
        panic!("expected assess hooks");
    };
    assert_eq!(parsed.incident_id.as_deref(), Some("abc123"));
}
