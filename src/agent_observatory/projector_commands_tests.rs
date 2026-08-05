use super::{CommandProjectionOutcome, CommandProjectionSkipReason, project_command_log};
use crate::agent_observatory::identity::{event_key, run_key};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{
    AgentEventKind, EvidenceTrustLevel, RepositoryUpsert, RepositoryWorktreeUpsert,
    reconcile_repository,
};
use crate::db::{LogEntry, init_pool};

const VERIFIED_PATH: &str = "/workspace/cortex/.worktrees/verified";
const VERIFIED_CWD: &str = "/workspace/cortex/.worktrees/verified/src/agent_observatory";
const CLAIMED_PATH: &str = "/workspace/cortex/.worktrees/claimed";
const CLAIMED_CWD: &str = "/workspace/cortex/.worktrees/claimed/crates/client";
const HEAD: &str = "0123456789012345678901234567890123456789";

fn repository() -> RepositoryUpsert {
    RepositoryUpsert {
        repository_key: "repo-key".to_string(),
        hostname: "dookie".to_string(),
        common_git_dir: "/workspace/cortex/.git".to_string(),
        primary_path: VERIFIED_PATH.to_string(),
        display_name: "cortex".to_string(),
        remote_url_hash: None,
        metadata_json: "{}".to_string(),
    }
}

fn worktree(key: &str, path: &str, branch: &str) -> RepositoryWorktreeUpsert {
    RepositoryWorktreeUpsert {
        worktree_key: key.to_string(),
        hostname: "dookie".to_string(),
        path: path.to_string(),
        git_dir: format!("{path}/.git"),
        branch_ref: Some(format!("refs/heads/{branch}")),
        branch_name: Some(branch.to_string()),
        head_sha: Some(HEAD.to_string()),
        upstream_ref: None,
        detached: false,
        bare: false,
        locked: false,
        lock_reason: None,
        prunable: false,
        prune_reason: None,
        dirty: false,
        staged_count: 0,
        unstaged_count: 0,
        untracked_count: 0,
        ahead: None,
        behind: None,
        status_hash: Some("clean".to_string()),
    }
}

fn agent_command() -> LogEntry {
    LogEntry {
        id: 301,
        timestamp: "2026-08-05T12:00:00.000Z".to_string(),
        hostname: "dookie".to_string(),
        facility: Some("agent".to_string()),
        severity: "warning".to_string(),
        app_name: Some("Claude".to_string()),
        process_id: Some("4242".to_string()),
        message: "curl --token [REDACTED]".to_string(),
        received_at: "2026-08-05T12:00:01.000Z".to_string(),
        source_ip: "agent-command://dookie/claude/claude-session".to_string(),
        ai_tool: Some("Claude".to_string()),
        ai_project: Some(VERIFIED_CWD.to_string()),
        ai_session_id: Some("claude-session".to_string()),
        ai_transcript_path: None,
        metadata_json: Some(
            r#"{
                "source_type":"agent_command",
                "source_kind":"agent-command",
                "agent_command":{
                    "schema_version":1,
                    "agent":"Claude",
                    "command_surface":"shell",
                    "cwd":"/workspace/cortex/.worktrees/verified/src/agent_observatory",
                    "pid":4242,
                    "exit_status":2,
                    "duration_ms":600000,
                    "finished_at":"2026-08-05T12:10:00.000Z",
                    "session_id":"claude-session"
                },
                "content_scrubbed":true
            }"#
            .to_string(),
        ),
    }
}

fn atuin() -> LogEntry {
    LogEntry {
        id: 302,
        timestamp: "2026-08-05T12:05:00.000Z".to_string(),
        hostname: "dookie".to_string(),
        facility: Some("shell".to_string()),
        severity: "info".to_string(),
        app_name: Some("atuin".to_string()),
        process_id: None,
        message: "git status".to_string(),
        received_at: "2026-08-05T12:05:01.000Z".to_string(),
        source_ip: "shell-history://dookie/user/atuin".to_string(),
        ai_tool: None,
        ai_project: Some(CLAIMED_CWD.to_string()),
        ai_session_id: Some("atuin-session".to_string()),
        ai_transcript_path: None,
        metadata_json: Some(
            r#"{
                "source_type":"shell_history",
                "source_kind":"shell-history",
                "shell":{
                    "name":"atuin",
                    "cwd":"/workspace/cortex/.worktrees/claimed/crates/client",
                    "session":"atuin-session",
                    "exit_status":0,
                    "duration_ms":250,
                    "timestamp_quality":"atuin_sqlite"
                },
                "content_scrubbed":true
            }"#
            .to_string(),
        ),
    }
}

#[test]
fn nested_agent_and_atuin_cwds_project_without_primary_override() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("commands.db"))).unwrap();
    let topology = reconcile_repository(
        &pool,
        &repository(),
        &[
            worktree("verified-worktree", VERIFIED_PATH, "verified"),
            worktree("claimed-worktree", CLAIMED_PATH, "claimed"),
        ],
        "2026-08-05T11:59:00.000Z",
    )
    .unwrap();
    let verified_id = topology
        .worktrees
        .iter()
        .find(|row| row.worktree_key == "verified-worktree")
        .unwrap()
        .id;
    let claimed_id = topology
        .worktrees
        .iter()
        .find(|row| row.worktree_key == "claimed-worktree")
        .unwrap()
        .id;

    let CommandProjectionOutcome::Projected(agent) =
        project_command_log(&pool, &agent_command()).unwrap()
    else {
        panic!("agent command should project");
    };
    assert_eq!(
        agent.run.run_key,
        run_key("dookie", "claude", "claude-session").unwrap()
    );
    assert_eq!(agent.run.primary_worktree_id, Some(verified_id));
    assert_eq!(agent.run.started_at, "2026-08-05T12:00:00.000Z");
    assert_eq!(agent.run.last_activity_at, "2026-08-05T12:10:00.000Z");
    assert_eq!(agent.event.event_kind, AgentEventKind::Command);
    assert_eq!(agent.event.severity, "warning");
    assert!(agent.event.content_scrubbed);
    assert_eq!(
        agent.event.event_key,
        event_key("logs", "301", "agent_command").unwrap()
    );
    let payload: serde_json::Value = serde_json::from_str(&agent.event.payload_json).unwrap();
    assert_eq!(payload["command"], "curl --token [REDACTED]");
    assert_eq!(payload["exit_status"], 2);
    assert_eq!(payload["duration_ms"], 600000);
    assert_eq!(payload["content_scrubbed"], true);

    let CommandProjectionOutcome::Projected(shell) = project_command_log(&pool, &atuin()).unwrap()
    else {
        panic!("overlapping Atuin command should project");
    };
    assert_eq!(shell.run.id, agent.run.id);
    assert_eq!(shell.run.primary_worktree_id, Some(verified_id));
    assert_eq!(shell.event.worktree_id, Some(claimed_id));
    assert_eq!(shell.event.event_kind, AgentEventKind::ShellHistory);
    assert_eq!(shell.event.severity, "info");
    assert_eq!(
        shell.event.event_key,
        event_key("logs", "302", "shell_history").unwrap()
    );
    let payload: serde_json::Value = serde_json::from_str(&shell.event.payload_json).unwrap();
    assert_eq!(payload["exit_status"], 0);
    assert_eq!(payload["duration_ms"], 250);
    assert_eq!(payload["shell_session_id"], "atuin-session");
    let outbox_payload: serde_json::Value =
        serde_json::from_str(&shell.outbox.as_ref().unwrap().payload_json).unwrap();
    assert_eq!(outbox_payload["event_kind"], "shell_history");
    assert_eq!(outbox_payload["source"], "shell_history");

    let connection = pool.get().unwrap();
    let evidence = connection
        .prepare(
            "SELECT evidence_kind, trust_level, confidence, is_primary, worktree_id
               FROM agent_run_worktrees WHERE run_id = ?1 ORDER BY id",
        )
        .unwrap()
        .query_map([agent.run.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        evidence,
        vec![
            (
                "agent_command_cwd".to_string(),
                EvidenceTrustLevel::Verified.as_str().to_string(),
                0.98,
                1,
                verified_id,
            ),
            (
                "atuin_cwd_window".to_string(),
                EvidenceTrustLevel::Claimed.as_str().to_string(),
                0.85,
                0,
                claimed_id,
            ),
        ]
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT event_count FROM agent_runs WHERE id = ?1",
                [agent.run.id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_stream_outbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        2
    );
    drop(connection);

    for row in [agent_command(), atuin()] {
        let CommandProjectionOutcome::Projected(replay) = project_command_log(&pool, &row).unwrap()
        else {
            panic!("valid replay should project");
        };
        assert!(!replay.event_inserted);
        assert!(!replay.materialized_state_changed);
        assert!(replay.outbox.is_none());
    }
}

#[test]
fn cwd_prefix_collision_is_not_attributed_to_a_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("prefix.db"))).unwrap();
    reconcile_repository(
        &pool,
        &repository(),
        &[worktree("verified-worktree", VERIFIED_PATH, "verified")],
        "2026-08-05T11:59:00.000Z",
    )
    .unwrap();

    let collision = format!("{VERIFIED_PATH}-other/src");
    let mut row = agent_command();
    row.id = 303;
    row.ai_project = Some(collision.clone());
    let mut metadata: serde_json::Value =
        serde_json::from_str(row.metadata_json.as_deref().unwrap()).unwrap();
    metadata["agent_command"]["cwd"] = serde_json::Value::String(collision);
    row.metadata_json = Some(metadata.to_string());

    let CommandProjectionOutcome::Skipped(diagnostic) = project_command_log(&pool, &row).unwrap()
    else {
        panic!("a sibling path prefix must not resolve to the worktree");
    };
    assert_eq!(
        diagnostic.reason,
        CommandProjectionSkipReason::NoMatchingWorktree
    );
    let connection = pool.get().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_runs", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn atuin_without_one_overlapping_run_is_skipped_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("unmatched.db"))).unwrap();
    reconcile_repository(
        &pool,
        &repository(),
        &[worktree("claimed-worktree", CLAIMED_PATH, "claimed")],
        "2026-08-05T11:59:00.000Z",
    )
    .unwrap();

    let CommandProjectionOutcome::Skipped(diagnostic) =
        project_command_log(&pool, &atuin()).unwrap()
    else {
        panic!("unmatched Atuin row must skip");
    };
    assert_eq!(
        diagnostic.reason,
        CommandProjectionSkipReason::NoOverlappingRun
    );
    let connection = pool.get().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_run_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}
