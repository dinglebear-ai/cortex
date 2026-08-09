use super::{SourceProjectionOutcome, SourceProjectionSkipReason, project_agent_source};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{
    AgentEventKind, AgentHookSourceRow, AgentLlmSourceRow, AgentMcpSourceRow, AgentSkillSourceRow,
    AgentSourceRecord, RepositoryUpsert, RepositoryWorktreeUpsert, reconcile_repository,
};
use crate::db::{LogBatchEntry, init_pool, insert_logs_batch};

const PROJECT: &str = "/workspace/cortex/.worktrees/ao038";
const HEAD: &str = "0123456789012345678901234567890123456789";

fn log_entry() -> LogBatchEntry {
    LogBatchEntry {
        timestamp: "2026-08-05T12:00:00.000Z".to_string(),
        hostname: "devhost".to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some("fixture".to_string()),
        process_id: None,
        message: "skill fixture".to_string(),
        raw: "skill fixture".to_string(),
        source_ip: "test://ao038-projector".to_string(),
        docker_checkpoint: None,
        ai_tool: Some("claude".to_string()),
        ai_project: Some(PROJECT.to_string()),
        ai_session_id: Some("session-one".to_string()),
        ai_transcript_path: None,
        metadata_json: Some("{}".to_string()),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

fn setup() -> (crate::db::DbPool, tempfile::TempDir, i64) {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("projection.db"))).unwrap();
    reconcile_repository(
        &pool,
        &RepositoryUpsert {
            repository_key: "repo-key".to_string(),
            hostname: "devhost".to_string(),
            common_git_dir: "/workspace/cortex/.git".to_string(),
            primary_path: "/workspace/cortex".to_string(),
            display_name: "cortex".to_string(),
            remote_url_hash: None,
            metadata_json: "{}".to_string(),
        },
        &[RepositoryWorktreeUpsert {
            worktree_key: "worktree-key".to_string(),
            hostname: "devhost".to_string(),
            path: PROJECT.to_string(),
            git_dir: format!("{PROJECT}/.git"),
            branch_ref: Some("refs/heads/main".to_string()),
            branch_name: Some("main".to_string()),
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
        }],
        "2026-08-05T12:00:00.000Z",
    )
    .unwrap();
    insert_logs_batch(&pool, &[log_entry()]).unwrap();
    let log_id = pool
        .get()
        .unwrap()
        .query_row("SELECT MAX(id) FROM logs", [], |row| row.get(0))
        .unwrap();
    (pool, dir, log_id)
}

fn records(log_id: i64) -> Vec<AgentSourceRecord> {
    vec![
        AgentSourceRecord::Mcp(AgentMcpSourceRow {
            cursor_id: 11,
            call_log_id: None,
            result_log_id: None,
            ai_tool: "claude".to_string(),
            ai_project: Some(PROJECT.to_string()),
            ai_session_id: Some("session-one".to_string()),
            hostname: "devhost".to_string(),
            timestamp: "2026-08-05T12:01:00.000Z".to_string(),
            turn_id: Some("turn-one".to_string()),
            call_id: "call-one".to_string(),
            tool_name: "mcp__server__tool".to_string(),
            mcp_server: Some("server".to_string()),
            mcp_tool: Some("tool".to_string()),
            event_kind: "call".to_string(),
            status: Some("ok".to_string()),
            duration_ms: Some(25),
            is_error: Some(false),
            arguments_json: Some("{}".to_string()),
            output_preview: None,
            error_text: None,
            metadata_json: Some("{}".to_string()),
        }),
        AgentSourceRecord::Hook(AgentHookSourceRow {
            cursor_id: 12,
            log_id: None,
            ai_tool: "claude".to_string(),
            ai_project: Some(PROJECT.to_string()),
            ai_session_id: Some("session-one".to_string()),
            hostname: "devhost".to_string(),
            timestamp: "2026-08-05T12:02:00.000Z".to_string(),
            hook_event: "post_tool".to_string(),
            hook_name: Some("audit".to_string()),
            hook_source: Some("settings".to_string()),
            hook_command: Some("echo scrubbed".to_string()),
            status: "success".to_string(),
            exit_code: Some(0),
            duration_ms: Some(10),
            stdout_preview: None,
            stderr_preview: None,
            persisted_output_path: None,
            trusted_hash: Some("sha256:test".to_string()),
            evidence_kind: "runtime".to_string(),
            metadata_json: Some("{}".to_string()),
        }),
        AgentSourceRecord::Skill(AgentSkillSourceRow {
            cursor_id: 13,
            log_id,
            ai_tool: "claude".to_string(),
            ai_project: Some(PROJECT.to_string()),
            ai_session_id: Some("session-one".to_string()),
            hostname: "devhost".to_string(),
            timestamp: "2026-08-05T12:03:00.000Z".to_string(),
            skill_name: "pdfs".to_string(),
            skill_plugin: Some("core".to_string()),
            event_kind: "invoked".to_string(),
            evidence_kind: "tag".to_string(),
        }),
        AgentSourceRecord::Llm(AgentLlmSourceRow {
            id: "llm-one".to_string(),
            started_at: "2026-08-05T12:04:00.000Z".to_string(),
            finished_at: Some("2026-08-05T12:04:01.000Z".to_string()),
            duration_ms: Some(1000),
            caller_surface: "cli".to_string(),
            action: "summarize".to_string(),
            provider: "openai".to_string(),
            model: Some("gpt-test".to_string()),
            program: None,
            incident_id: None,
            ai_tool: Some("claude".to_string()),
            ai_project: Some(PROJECT.to_string()),
            ai_session_id: Some("session-one".to_string()),
            evidence_counts_json: Some("{}".to_string()),
            prompt_bytes: Some(20),
            output_bytes: Some(40),
            status: "success".to_string(),
            error: None,
            metadata_json: Some("{}".to_string()),
        }),
    ]
}

#[test]
fn four_source_types_project_typed_events_and_replay_without_duplicates() {
    let (pool, _dir, log_id) = setup();
    let mut results = Vec::new();
    for record in records(log_id) {
        let SourceProjectionOutcome::Projected(result) =
            project_agent_source(&pool, &record).unwrap()
        else {
            panic!("source should project");
        };
        assert!(result.event_inserted);
        results.push(result);
    }
    assert_eq!(results[0].event.event_kind, AgentEventKind::Mcp);
    assert_eq!(results[1].event.event_kind, AgentEventKind::Hook);
    assert_eq!(results[2].event.event_kind, AgentEventKind::Skill);
    assert_eq!(results[3].event.event_kind, AgentEventKind::Llm);
    assert_eq!(
        results
            .iter()
            .map(|result| result.event.source_id.as_str())
            .collect::<Vec<_>>(),
        vec!["11", "12", "13", "llm-one"]
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.event.source_kind.as_str())
            .collect::<Vec<_>>(),
        vec![
            "mcp_events",
            "hook_events",
            "skill_events",
            "llm_invocations"
        ]
    );
    assert!(results.iter().all(|result| result.actor.is_some()));
    assert!(
        results
            .iter()
            .all(|result| result.run.provider_tool.as_deref() == Some("claude"))
    );
    assert_eq!(
        results
            .iter()
            .map(|result| {
                result
                    .actor
                    .as_ref()
                    .and_then(|actor| actor.display_name.as_deref())
            })
            .collect::<Vec<_>>(),
        vec![
            Some("server/tool"),
            Some("audit"),
            Some("core/pdfs"),
            Some("openai/gpt-test")
        ]
    );
    let run_id = results[0].run.id;
    assert!(results.iter().all(|result| result.run.id == run_id));

    for record in records(log_id) {
        let SourceProjectionOutcome::Projected(result) =
            project_agent_source(&pool, &record).unwrap()
        else {
            panic!("replay should project idempotently");
        };
        assert!(!result.event_inserted);
        assert!(result.outbox.is_none());
    }
    let conn = pool.get().unwrap();
    let counts: (i64, i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM agent_run_events),
                (SELECT COUNT(*) FROM agent_run_actors),
                (SELECT COUNT(*) FROM agent_stream_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (4, 4, 4));
    let verified_project_evidence: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_run_worktrees
              WHERE run_id = ?1
                AND evidence_kind = 'transcript_project_path'
                AND trust_level = 'verified'
                AND confidence = 0.95
                AND is_primary = 1",
            [run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(verified_project_evidence, 3);
}

#[test]
fn unknown_source_event_is_typed_payload_is_capped_and_missing_session_skips() {
    let (pool, _dir, log_id) = setup();
    let mut record = records(log_id).remove(0);
    let AgentSourceRecord::Mcp(row) = &mut record else {
        unreachable!()
    };
    row.event_kind = "future-provider-shape".to_string();
    row.output_preview = Some("x".repeat(100_000));
    let SourceProjectionOutcome::Projected(result) = project_agent_source(&pool, &record).unwrap()
    else {
        panic!("unknown source event should use fallback");
    };
    assert_eq!(result.event.event_kind, AgentEventKind::Mcp);
    assert!(result.event.payload_json.len() <= 16 * 1024);
    assert!(result.event.payload_json.contains("future-provider-shape"));

    let AgentSourceRecord::Mcp(row) = &mut record else {
        unreachable!()
    };
    row.cursor_id = 15;
    row.ai_session_id = None;
    let SourceProjectionOutcome::Skipped(diagnostic) =
        project_agent_source(&pool, &record).unwrap()
    else {
        panic!("missing session should skip");
    };
    assert_eq!(
        diagnostic.reason,
        SourceProjectionSkipReason::MissingSession
    );
}
