use super::{
    HeadObservation, get_agent_backfill, legacy_transition_commits, run_agent_backfill_chunk,
    start_agent_backfill,
};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{
    GitCommitUpsert, RepositoryUpsert, RepositoryWorktreeUpsert, projection_cursor,
    reconcile_git_commits, reconcile_repository,
};
use crate::db::{DbPool, LogBatchEntry, init_pool, insert_logs_batch, page_agent_projection_logs};
use tempfile::TempDir;

const PROJECT: &str = "/workspace/cortex/.worktrees/ao040";
const COMMAND_CWD: &str = "/workspace/cortex/.worktrees/ao040/src";
const HEAD: &str = "0123456789012345678901234567890123456789";
const MID: &str = "1111111111111111111111111111111111111111";
const TIP: &str = "2222222222222222222222222222222222222222";

fn log_entry(timestamp: &str, message: &str, transcript: bool) -> LogBatchEntry {
    let metadata_json = if transcript {
        r#"{"role":"assistant"}"#.to_string()
    } else {
        format!(
            r#"{{
                "source_type":"agent_command",
                "source_kind":"agent-command",
                "agent_command":{{
                    "schema_version":1,
                    "agent":"Claude",
                    "command_surface":"shell",
                    "cwd":"{COMMAND_CWD}",
                    "pid":4242,
                    "exit_status":0,
                    "duration_ms":1000,
                    "finished_at":"2026-08-05T12:01:01.000Z",
                    "session_id":"session-one"
                }},
                "content_scrubbed":true
            }}"#
        )
    };
    LogBatchEntry {
        timestamp: timestamp.to_string(),
        hostname: "devhost".to_string(),
        facility: Some("agent".to_string()),
        severity: "info".to_string(),
        app_name: Some("Claude".to_string()),
        process_id: Some("4242".to_string()),
        message: message.to_string(),
        raw: message.to_string(),
        source_ip: if transcript {
            "agent-ai-transcript://devhost".to_string()
        } else {
            "agent-command://devhost/claude/session-one".to_string()
        },
        docker_checkpoint: None,
        ai_tool: Some("Claude".to_string()),
        ai_project: Some(if transcript { PROJECT } else { COMMAND_CWD }.to_string()),
        ai_session_id: Some("session-one".to_string()),
        ai_transcript_path: transcript.then(|| "/tmp/session-one.jsonl".to_string()),
        metadata_json: Some(metadata_json),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

fn setup(path: &std::path::Path) -> DbPool {
    let pool = init_pool(&StorageConfig::for_test(path.to_path_buf())).unwrap();
    reconcile_repository(
        &pool,
        &RepositoryUpsert {
            repository_key: "repo-key".to_string(),
            hostname: "devhost".to_string(),
            common_git_dir: "/workspace/cortex/.git".to_string(),
            primary_path: PROJECT.to_string(),
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
        "2026-08-05T11:59:00.000Z",
    )
    .unwrap();
    insert_logs_batch(
        &pool,
        &[
            log_entry("2026-08-05T12:00:00.000Z", "transcript", true),
            log_entry("2026-08-05T12:01:00.000Z", "git status", false),
        ],
    )
    .unwrap();
    pool
}

fn finish(pool: &DbPool, job_id: i64, budget: usize) -> super::AgentBackfillJob {
    let mut job = get_agent_backfill(pool, job_id).unwrap();
    for _ in 0..32 {
        if job.progress.done {
            break;
        }
        job = run_agent_backfill_chunk(pool, job_id, budget).unwrap();
    }
    assert!(job.progress.done);
    job
}

fn snapshot(pool: &DbPool) -> (Vec<String>, Vec<String>) {
    let connection = pool.get().unwrap();
    let runs = connection
        .prepare("SELECT run_key FROM agent_runs ORDER BY run_key")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<String>>>()
        .unwrap();
    let events = connection
        .prepare("SELECT event_key FROM agent_run_events ORDER BY event_key")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<String>>>()
        .unwrap();
    (runs, events)
}

#[test]
fn cancelled_backfill_resumes_from_durable_progress_and_matches_uninterrupted() {
    let resumed_dir = TempDir::new().unwrap();
    let resumed_path = resumed_dir.path().join("resumed.db");
    let pool = setup(&resumed_path);
    let started = start_agent_backfill(&pool).unwrap();
    assert_eq!(started.progress.high_water.logs, 2);
    let partial = run_agent_backfill_chunk(&pool, started.job_id, 1).unwrap();
    assert!(!partial.progress.done);
    assert_eq!(partial.progress.cursors.logs, 1);
    drop(pool);

    let reopened = init_pool(&StorageConfig::for_test(resumed_path)).unwrap();
    let durable = get_agent_backfill(&reopened, started.job_id).unwrap();
    assert_eq!(durable.progress.cursors.logs, 1);
    let resumed = finish(&reopened, started.job_id, 1);
    assert_eq!(resumed.status, "done");
    assert_eq!(resumed.progress.source_rows_scanned, 2);
    assert_eq!(projection_cursor(&reopened, "logs").unwrap(), "");
    let resumed_snapshot = snapshot(&reopened);

    let uninterrupted_dir = TempDir::new().unwrap();
    let uninterrupted = setup(&uninterrupted_dir.path().join("uninterrupted.db"));
    let job = start_agent_backfill(&uninterrupted).unwrap();
    let completed = finish(&uninterrupted, job.job_id, 500);
    assert_eq!(completed.progress.source_rows_scanned, 2);
    assert_eq!(snapshot(&uninterrupted), resumed_snapshot);
}

#[test]
fn live_row_after_high_water_is_projected_once_and_not_consumed_by_backfill() {
    let dir = TempDir::new().unwrap();
    let pool = setup(&dir.path().join("live.db"));
    let job = start_agent_backfill(&pool).unwrap();
    assert_eq!(job.progress.high_water.logs, 2);

    insert_logs_batch(
        &pool,
        &[log_entry(
            "2026-08-05T12:02:00.000Z",
            "live transcript",
            true,
        )],
    )
    .unwrap();
    let live = page_agent_projection_logs(&pool, 2, 1).unwrap().remove(0);
    assert_eq!(projection_cursor(&pool, "logs").unwrap(), "");
    crate::agent_observatory::projector::project_log_row_with_cursor(&pool, &live).unwrap();
    assert_eq!(projection_cursor(&pool, "logs").unwrap(), "3");

    let completed = finish(&pool, job.job_id, 1);
    assert_eq!(completed.progress.source_rows_scanned, 2);
    assert_eq!(projection_cursor(&pool, "logs").unwrap(), "3");
    let connection = pool.get().unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_run_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    let distinct: i64 = connection
        .query_row(
            "SELECT COUNT(DISTINCT event_key) FROM agent_run_events",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((count, distinct), (3, 3));
}

#[test]
fn legacy_head_observation_recovers_entire_commit_range_from_durable_graph() {
    let dir = TempDir::new().unwrap();
    let pool = setup(&dir.path().join("legacy-head.db"));
    let repository_id = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT id FROM repositories WHERE repository_key = 'repo-key'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let commits = reconcile_git_commits(
        &pool,
        "repo-key",
        &[
            GitCommitUpsert {
                sha: HEAD.to_string(),
                parent_shas_json: "[]".to_string(),
                author_name: None,
                author_email_hash: None,
                authored_at: None,
                committed_at: None,
                subject: "base".to_string(),
                changed_files: Some(1),
                insertions: Some(1),
                deletions: Some(0),
                changed_paths_json: "[]".to_string(),
                reachable: true,
                metadata_json: "{}".to_string(),
            },
            GitCommitUpsert {
                sha: MID.to_string(),
                parent_shas_json: format!(r#"["{HEAD}"]"#),
                author_name: None,
                author_email_hash: None,
                authored_at: None,
                committed_at: None,
                subject: "middle".to_string(),
                changed_files: Some(1),
                insertions: Some(1),
                deletions: Some(0),
                changed_paths_json: "[]".to_string(),
                reachable: true,
                metadata_json: "{}".to_string(),
            },
            GitCommitUpsert {
                sha: TIP.to_string(),
                parent_shas_json: format!(r#"["{MID}"]"#),
                author_name: None,
                author_email_hash: None,
                authored_at: None,
                committed_at: None,
                subject: "tip".to_string(),
                changed_files: Some(1),
                insertions: Some(1),
                deletions: Some(0),
                changed_paths_json: "[]".to_string(),
                reachable: true,
                metadata_json: "{}".to_string(),
            },
        ],
        &[],
        "2026-08-05T12:02:00.000Z",
    )
    .unwrap();
    assert_eq!(commits.len(), 3);
    let observation = HeadObservation {
        id: 1,
        observation_key: "legacy-head".to_string(),
        repository_id,
        worktree_id: None,
        observed_at: "2026-08-05T12:02:00.000Z".to_string(),
        old_head_sha: Some(HEAD.to_string()),
        new_head_sha: Some(TIP.to_string()),
        payload_json: format!(r#"{{"head_sha":"{TIP}","old_head_sha":"{HEAD}"}}"#),
    };
    assert_eq!(
        legacy_transition_commits(&pool, &observation)
            .unwrap()
            .into_iter()
            .map(|commit| commit.sha)
            .collect::<Vec<_>>(),
        vec![MID.to_string(), TIP.to_string()]
    );
}
