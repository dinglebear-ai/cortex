use super::{TranscriptProjectionOutcome, project_transcript_log};
use crate::agent_observatory::identity::{event_key, run_key};
use crate::config::StorageConfig;
use crate::db::{LogEntry, init_pool};
use rusqlite::Connection;

fn log(
    id: i64,
    timestamp: &str,
    tool: &str,
    project: &str,
    session: Option<&str>,
    path: &str,
    message: &str,
) -> LogEntry {
    LogEntry {
        id,
        timestamp: timestamp.to_string(),
        hostname: "dookie".to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some(format!("{tool}-transcript")),
        process_id: Some(format!("pid-{id}")),
        message: message.to_string(),
        received_at: timestamp.to_string(),
        source_ip: "agent-ai-transcript://dookie".to_string(),
        ai_tool: Some(tool.to_string()),
        ai_project: Some(project.to_string()),
        ai_session_id: session.map(str::to_string),
        ai_transcript_path: Some(path.to_string()),
        metadata_json: Some(r#"{"role":"assistant"}"#.to_string()),
    }
}

fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn claude_codex_and_gemini_rows_project_expected_runs_events_and_replay_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("transcripts.db"))).unwrap();
    let rows = vec![
        log(
            101,
            "2026-08-05T12:00:00.000Z",
            "Claude",
            "/workspace/cortex/.claude/worktrees/task-one",
            Some("claude-session"),
            "/home/user/.claude/projects/cortex/claude-session.jsonl",
            "Claude first response",
        ),
        log(
            104,
            "2026-08-05T12:05:00.000Z",
            "claude",
            "/workspace/cortex/.worktrees/task-one",
            Some("claude-session"),
            "/home/user/.claude/projects/cortex/claude-session.jsonl",
            "Claude second response",
        ),
        log(
            102,
            "2026-08-05T12:01:00.000Z",
            "Codex",
            "/workspace/cortex/.worktrees/task-two",
            Some("codex-session"),
            "/home/user/.codex/sessions/codex-session.jsonl",
            "Codex response",
        ),
        log(
            103,
            "2026-08-05T12:02:00.000Z",
            "Gemini",
            "/workspace/gemini-project",
            Some("gemini-session"),
            "/home/user/.gemini/tmp/gemini-session.json",
            "Gemini response",
        ),
    ];

    let mut projected = Vec::new();
    for row in &rows {
        let TranscriptProjectionOutcome::Projected(result) =
            project_transcript_log(&pool, row).unwrap()
        else {
            panic!("valid provider row should project");
        };
        assert!(result.event_inserted);
        assert!(result.outbox.is_some());
        assert_eq!(
            result.event.event_key,
            event_key("logs", &row.id.to_string(), "transcript").unwrap()
        );
        assert_eq!(result.event.observed_at, row.timestamp);
        assert_eq!(result.run.transcript_path, row.ai_transcript_path);
        projected.push(result);
    }

    assert_eq!(projected[0].run.id, projected[1].run.id);
    assert_eq!(projected[1].run.event_count, 2);
    assert_eq!(projected[1].run.started_at, "2026-08-05T12:00:00.000Z");
    assert_eq!(
        projected[1].run.last_activity_at,
        "2026-08-05T12:05:00.000Z"
    );
    assert_eq!(
        projected[0].run.run_key,
        run_key("dookie", "claude", "claude-session").unwrap()
    );
    assert_eq!(
        projected[2].run.run_key,
        run_key("dookie", "codex", "codex-session").unwrap()
    );
    assert_eq!(
        projected[3].run.run_key,
        run_key("dookie", "gemini", "gemini-session").unwrap()
    );
    let payload: serde_json::Value =
        serde_json::from_str(&projected[0].event.payload_json).unwrap();
    assert_eq!(payload["project"], "/workspace/cortex");
    assert_eq!(payload["message"], "Claude first response");
    assert_eq!(
        payload["transcript_path"],
        rows[0].ai_transcript_path.as_deref().unwrap()
    );

    let connection = pool.get().unwrap();
    assert_eq!(count(&connection, "agent_runs"), 3);
    assert_eq!(count(&connection, "agent_run_events"), 4);
    assert_eq!(count(&connection, "agent_stream_outbox"), 4);
    drop(connection);

    for row in &rows {
        let TranscriptProjectionOutcome::Projected(replay) =
            project_transcript_log(&pool, row).unwrap()
        else {
            panic!("valid replay should project");
        };
        assert!(!replay.event_inserted);
        assert!(!replay.materialized_state_changed);
        assert!(replay.outbox.is_none());
    }
    let connection = pool.get().unwrap();
    assert_eq!(count(&connection, "agent_runs"), 3);
    assert_eq!(count(&connection, "agent_run_events"), 4);
    assert_eq!(count(&connection, "agent_stream_outbox"), 4);
}

#[test]
fn missing_session_and_malformed_metadata_are_skipped_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("skips.db"))).unwrap();
    let missing = log(
        201,
        "2026-08-05T13:00:00.000Z",
        "claude",
        "/workspace/cortex",
        None,
        "/tmp/missing.jsonl",
        "missing session",
    );
    let TranscriptProjectionOutcome::Skipped(diagnostic) =
        project_transcript_log(&pool, &missing).unwrap()
    else {
        panic!("missing session must skip");
    };
    assert_eq!(diagnostic.log_id, 201);

    let mut malformed = log(
        202,
        "2026-08-05T13:01:00.000Z",
        "gemini",
        "/workspace/gemini",
        Some("gemini-session"),
        "/tmp/gemini.json",
        "bad metadata",
    );
    malformed.metadata_json = Some("{".to_string());
    assert!(matches!(
        project_transcript_log(&pool, &malformed).unwrap(),
        TranscriptProjectionOutcome::Skipped(_)
    ));

    let connection = pool.get().unwrap();
    assert_eq!(count(&connection, "agent_runs"), 0);
    assert_eq!(count(&connection, "agent_run_events"), 0);
    assert_eq!(count(&connection, "agent_stream_outbox"), 0);
}
