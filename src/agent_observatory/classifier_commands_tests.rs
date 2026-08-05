use super::{CommandLogClassification, CommandLogSource, CommandSkipReason, classify_command_log};
use crate::db::LogEntry;

fn base_log(source_ip: &str, metadata_json: &str) -> LogEntry {
    LogEntry {
        id: 301,
        timestamp: "2026-08-05T12:00:00.000Z".to_string(),
        hostname: "dookie".to_string(),
        facility: Some("agent".to_string()),
        severity: "warning".to_string(),
        app_name: Some("claude".to_string()),
        process_id: Some("4242".to_string()),
        message: "curl --token [REDACTED]".to_string(),
        received_at: "2026-08-05T12:00:01.000Z".to_string(),
        source_ip: source_ip.to_string(),
        ai_tool: Some("Claude".to_string()),
        ai_project: Some("/workspace/cortex/.worktrees/verified".to_string()),
        ai_session_id: Some("claude-session".to_string()),
        ai_transcript_path: None,
        metadata_json: Some(metadata_json.to_string()),
    }
}

#[test]
fn agent_command_shape_preserves_verified_command_fields() {
    let row = base_log(
        "agent-command://dookie/claude/claude-session",
        r#"{
            "source_type":"agent_command",
            "source_kind":"agent-command",
            "agent_command":{
                "schema_version":1,
                "agent":"Claude",
                "command_surface":"shell",
                "cwd":"/workspace/cortex/.worktrees/verified",
                "pid":4242,
                "exit_status":2,
                "duration_ms":600000,
                "finished_at":"2026-08-05T12:10:00.000Z",
                "session_id":"claude-session"
            },
            "content_scrubbed":true
        }"#,
    );
    let CommandLogClassification::Project(projected) = classify_command_log(&row) else {
        panic!("valid agent command should classify");
    };
    assert_eq!(projected.source, CommandLogSource::AgentCommand);
    assert_eq!(projected.tool.as_deref(), Some("claude"));
    assert_eq!(
        projected.provider_session_id.as_deref(),
        Some("claude-session")
    );
    assert_eq!(projected.cwd, "/workspace/cortex/.worktrees/verified");
    assert_eq!(projected.exit_status, Some(2));
    assert_eq!(projected.duration_ms, Some(600000));
    assert_eq!(
        projected.finished_at.as_deref(),
        Some("2026-08-05T12:10:00.000Z")
    );
    assert_eq!(projected.command_surface.as_deref(), Some("shell"));
    assert_eq!(projected.severity, "warning");
    assert!(projected.content_scrubbed);
    assert_eq!(projected.command, "curl --token [REDACTED]");
}

#[test]
fn atuin_shape_preserves_claimed_cwd_session_exit_and_duration() {
    let mut row = base_log(
        "shell-history://dookie/user/atuin",
        r#"{
            "source_type":"shell_history",
            "source_kind":"shell-history",
            "shell":{
                "name":"atuin",
                "cwd":"/workspace/cortex/.worktrees/claimed",
                "session":"atuin-session",
                "exit_status":0,
                "duration_ms":250,
                "timestamp_quality":"atuin_sqlite"
            },
            "content_scrubbed":true
        }"#,
    );
    row.id = 302;
    row.timestamp = "2026-08-05T12:05:00.000Z".to_string();
    row.received_at = "2026-08-05T12:05:01.000Z".to_string();
    row.facility = Some("shell".to_string());
    row.app_name = Some("atuin".to_string());
    row.process_id = None;
    row.message = "git status".to_string();
    row.ai_tool = None;
    row.ai_project = Some("/workspace/cortex/.worktrees/claimed".to_string());
    row.ai_session_id = Some("atuin-session".to_string());

    let CommandLogClassification::Project(projected) = classify_command_log(&row) else {
        panic!("valid Atuin row should classify");
    };
    assert_eq!(projected.source, CommandLogSource::Atuin);
    assert_eq!(projected.tool, None);
    assert_eq!(projected.shell_session_id.as_deref(), Some("atuin-session"));
    assert_eq!(projected.cwd, "/workspace/cortex/.worktrees/claimed");
    assert_eq!(projected.exit_status, Some(0));
    assert_eq!(projected.duration_ms, Some(250));
    assert_eq!(projected.severity, "warning");
    assert!(projected.content_scrubbed);
}

#[test]
fn finished_at_comparison_uses_the_actual_instant() {
    let row = base_log(
        "agent-command://dookie/claude/claude-session",
        r#"{
            "source_type":"agent_command",
            "agent_command":{
                "agent":"Claude",
                "cwd":"/workspace/cortex/.worktrees/verified",
                "finished_at":"2026-08-05T08:10:00.000-04:00",
                "duration_ms":600000,
                "session_id":"claude-session"
            },
            "content_scrubbed":true
        }"#,
    );
    let CommandLogClassification::Project(projected) = classify_command_log(&row) else {
        panic!("a later instant with an earlier local clock text should classify");
    };
    assert_eq!(
        projected.finished_at.as_deref(),
        Some("2026-08-05T08:10:00.000-04:00")
    );
}

#[test]
fn forwarded_atuin_shape_uses_root_fields_and_agent_prefix() {
    let mut row = base_log(
        "agent-shell-history://dookie",
        r#"{
            "source_type":"shell_history",
            "shell":"atuin",
            "cwd":"/workspace/cortex/.worktrees/claimed",
            "exit_status":7,
            "duration_ms":1250,
            "content_scrubbed":true
        }"#,
    );
    row.id = 303;
    row.app_name = Some("atuin".to_string());
    row.process_id = None;
    row.message = "cargo test".to_string();
    row.ai_tool = None;
    row.ai_project = None;
    row.ai_session_id = Some("forwarded-atuin-session".to_string());

    let CommandLogClassification::Project(projected) = classify_command_log(&row) else {
        panic!("forwarded Atuin row should classify");
    };
    assert_eq!(projected.source, CommandLogSource::Atuin);
    assert_eq!(
        projected.shell_session_id.as_deref(),
        Some("forwarded-atuin-session")
    );
    assert_eq!(projected.cwd, "/workspace/cortex/.worktrees/claimed");
    assert_eq!(projected.exit_status, Some(7));
    assert_eq!(projected.duration_ms, Some(1250));
    assert_eq!(projected.command, "cargo test");
    assert!(projected.content_scrubbed);
}

#[test]
fn mismatched_prefix_unscrubbed_content_and_malformed_metadata_are_skipped() {
    let mismatch = base_log(
        "shell-history://dookie/user/atuin",
        r#"{"source_type":"agent_command","agent_command":{},"content_scrubbed":true}"#,
    );
    let CommandLogClassification::Skip(diagnostic) = classify_command_log(&mismatch) else {
        panic!("source prefix mismatch must skip");
    };
    assert_eq!(diagnostic.reason, CommandSkipReason::SourceShapeMismatch);

    let unscrubbed = base_log(
        "agent-command://dookie/claude/session",
        r#"{
            "source_type":"agent_command",
            "agent_command":{
                "cwd":"/workspace/cortex/.worktrees/verified",
                "finished_at":"2026-08-05T12:10:00.000Z",
                "duration_ms":1
            },
            "content_scrubbed":false
        }"#,
    );
    let CommandLogClassification::Skip(diagnostic) = classify_command_log(&unscrubbed) else {
        panic!("unscrubbed command must skip");
    };
    assert_eq!(diagnostic.reason, CommandSkipReason::ContentNotScrubbed);

    let malformed = base_log("agent-command://dookie/claude/session", "{");
    let CommandLogClassification::Skip(diagnostic) = classify_command_log(&malformed) else {
        panic!("malformed metadata must skip");
    };
    assert_eq!(diagnostic.reason, CommandSkipReason::InvalidMetadataJson);
}
