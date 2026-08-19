use super::{active_at, attribute_exact_commits};
use crate::agent_observatory::projector::{CommandProjectionOutcome, project_command_log};
use crate::config::StorageConfig;
use crate::db::LogEntry;
use crate::db::agent_observatory::{
    AgentRunCommitUpsert, EvidenceTrustLevel, GitCommitUpsert, RepositoryUpsert,
    RepositoryWorktreeUpsert, list_agent_run_commits, reconcile_git_commits, reconcile_repository,
    upsert_agent_run_commit,
};
use crate::db::init_pool;

const PROJECT: &str = "/workspace/cortex/.worktrees/attribution";
const CWD: &str = "/workspace/cortex/.worktrees/attribution/src";
const OLD: &str = "0123456789012345678901234567890123456789";
const NEW: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

fn command() -> LogEntry {
    LogEntry {
        id: 1,
        timestamp: "2026-08-05T12:00:00.000Z".to_string(),
        hostname: "devhost".to_string(),
        facility: Some("agent".to_string()),
        severity: "info".to_string(),
        app_name: Some("Claude".to_string()),
        process_id: Some("4242".to_string()),
        message: "git commit".to_string(),
        received_at: "2026-08-05T12:00:01.000Z".to_string(),
        source_ip: "agent-command://devhost/claude/session-one".to_string(),
        ai_tool: Some("Claude".to_string()),
        ai_project: Some(CWD.to_string()),
        ai_session_id: Some("session-one".to_string()),
        ai_transcript_path: None,
        metadata_json: Some(format!(
            r#"{{
                "source_type":"agent_command",
                "source_kind":"agent-command",
                "agent_command":{{
                    "schema_version":1,
                    "agent":"Claude",
                    "command_surface":"shell",
                    "cwd":"{CWD}",
                    "pid":4242,
                    "exit_status":0,
                    "duration_ms":600000,
                    "finished_at":"2026-08-05T12:10:00.000Z",
                    "session_id":"session-one"
                }},
                "content_scrubbed":true
            }}"#
        )),
    }
}

#[test]
fn exact_commit_uses_direct_command_evidence_and_replays_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("attribution.db"))).unwrap();
    let topology = reconcile_repository(
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
            head_sha: Some(OLD.to_string()),
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
    let worktree_id = topology.worktrees[0].id;
    let CommandProjectionOutcome::Projected(projected) =
        project_command_log(&pool, &command()).unwrap()
    else {
        panic!("command should project");
    };
    let commits = reconcile_git_commits(
        &pool,
        "repo-key",
        &[GitCommitUpsert {
            sha: NEW.to_string(),
            parent_shas_json: format!(r#"["{OLD}"]"#),
            author_name: None,
            author_email_hash: None,
            authored_at: Some("2026-08-05T12:01:30.000Z".to_string()),
            committed_at: Some("2026-08-05T12:01:30.000Z".to_string()),
            subject: "exact commit".to_string(),
            changed_files: Some(1),
            insertions: Some(1),
            deletions: Some(0),
            changed_paths_json: "[]".to_string(),
            reachable: true,
            metadata_json: "{}".to_string(),
        }],
        &[],
        "2026-08-05T12:02:00.000Z",
    )
    .unwrap();
    assert_eq!(
        attribute_exact_commits(
            &pool,
            worktree_id,
            "head-observation",
            "2026-08-05T12:02:00.000Z",
            Some(OLD),
            NEW,
            &commits,
        )
        .unwrap(),
        1
    );
    assert_eq!(
        attribute_exact_commits(
            &pool,
            worktree_id,
            "head-observation",
            "2026-08-05T12:02:00.000Z",
            Some(OLD),
            NEW,
            &commits,
        )
        .unwrap(),
        1
    );

    let relations = list_agent_run_commits(&pool, projected.run.id).unwrap();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].commit_id, commits[0].id);
    assert_eq!(relations[0].evidence_kind, "agent_command_cwd");
    assert_eq!(relations[0].trust_level, EvidenceTrustLevel::Verified);
    assert!((relations[0].confidence - 0.98).abs() < f64::EPSILON);

    let connection = pool.get().unwrap();
    connection
        .execute("DELETE FROM agent_run_commits", [])
        .unwrap();
    connection
        .execute(
            "UPDATE agent_run_worktrees SET confidence = 0.50 WHERE run_id = ?1",
            [projected.run.id],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        attribute_exact_commits(
            &pool,
            worktree_id,
            "low-confidence-head",
            "2026-08-05T12:02:00.000Z",
            Some(OLD),
            NEW,
            &commits,
        )
        .unwrap(),
        0
    );
    assert!(
        list_agent_run_commits(&pool, projected.run.id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn future_activity_does_not_retroactively_activate_stale_run() {
    let observed = chrono::DateTime::parse_from_rfc3339("2026-08-05T12:10:00.000Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(
        !active_at(
            observed,
            "2026-08-05T12:00:00.000Z",
            "2026-08-05T13:00:00.000Z",
            None,
            "2026-08-05T12:00:00.000Z",
            "2026-08-05T13:00:00.000Z",
        )
        .unwrap()
    );
    assert!(
        active_at(
            observed,
            "2026-08-05T12:00:00.000Z",
            "2026-08-05T13:00:00.000Z",
            None,
            "2026-08-05T12:00:00.000Z",
            "2026-08-05T12:08:00.000Z",
        )
        .unwrap()
    );
}

#[test]
fn ambiguous_direct_runs_are_capped_to_correlated_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("ambiguous.db"))).unwrap();
    let topology = reconcile_repository(
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
            head_sha: Some(OLD.to_string()),
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
    let worktree_id = topology.worktrees[0].id;
    let CommandProjectionOutcome::Projected(first) =
        project_command_log(&pool, &command()).unwrap()
    else {
        panic!("first command should project");
    };
    let mut second_row = command();
    second_row.id = 2;
    second_row.ai_session_id = Some("session-two".to_string());
    second_row.source_ip = "agent-command://devhost/claude/session-two".to_string();
    second_row.metadata_json = second_row
        .metadata_json
        .take()
        .map(|metadata| metadata.replace("session-one", "session-two"));
    let CommandProjectionOutcome::Projected(second) =
        project_command_log(&pool, &second_row).unwrap()
    else {
        panic!("second command should project");
    };
    assert_ne!(first.run.id, second.run.id);

    let commits = reconcile_git_commits(
        &pool,
        "repo-key",
        &[GitCommitUpsert {
            sha: NEW.to_string(),
            parent_shas_json: format!(r#"["{OLD}"]"#),
            author_name: None,
            author_email_hash: None,
            authored_at: Some("2026-08-05T12:01:30.000Z".to_string()),
            committed_at: Some("2026-08-05T12:01:30.000Z".to_string()),
            subject: "ambiguous commit".to_string(),
            changed_files: Some(1),
            insertions: Some(1),
            deletions: Some(0),
            changed_paths_json: "[]".to_string(),
            reachable: true,
            metadata_json: "{}".to_string(),
        }],
        &[],
        "2026-08-05T12:02:00.000Z",
    )
    .unwrap();
    assert_eq!(
        attribute_exact_commits(
            &pool,
            worktree_id,
            "ambiguous-head",
            "2026-08-05T12:02:00.000Z",
            Some(OLD),
            NEW,
            &commits,
        )
        .unwrap(),
        2
    );
    for run_id in [first.run.id, second.run.id] {
        let relations = list_agent_run_commits(&pool, run_id).unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].evidence_kind, "git_head_overlap");
        assert_eq!(relations[0].trust_level, EvidenceTrustLevel::Correlated);
        assert!((relations[0].confidence - 0.75).abs() < f64::EPSILON);
    }
}

#[test]
fn run_commit_relation_rejects_cross_repository_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("cross-repository.db"),
    ))
    .unwrap();
    let topology = reconcile_repository(
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
            head_sha: Some(OLD.to_string()),
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
    let CommandProjectionOutcome::Projected(projected) =
        project_command_log(&pool, &command()).unwrap()
    else {
        panic!("command should project");
    };

    reconcile_repository(
        &pool,
        &RepositoryUpsert {
            repository_key: "foreign-repo-key".to_string(),
            hostname: "devhost".to_string(),
            common_git_dir: "/workspace/foreign/.git".to_string(),
            primary_path: "/workspace/foreign".to_string(),
            display_name: "foreign".to_string(),
            remote_url_hash: None,
            metadata_json: "{}".to_string(),
        },
        &[],
        "2026-08-05T11:59:00.000Z",
    )
    .unwrap();
    let foreign_commits = reconcile_git_commits(
        &pool,
        "foreign-repo-key",
        &[GitCommitUpsert {
            sha: "fedcbafedcbafedcbafedcbafedcbafedcbafedc".to_string(),
            parent_shas_json: "[]".to_string(),
            author_name: None,
            author_email_hash: None,
            authored_at: Some("2026-08-05T12:01:30.000Z".to_string()),
            committed_at: Some("2026-08-05T12:01:30.000Z".to_string()),
            subject: "foreign commit".to_string(),
            changed_files: Some(1),
            insertions: Some(1),
            deletions: Some(0),
            changed_paths_json: "[]".to_string(),
            reachable: true,
            metadata_json: "{}".to_string(),
        }],
        &[],
        "2026-08-05T12:02:00.000Z",
    )
    .unwrap();

    let error = upsert_agent_run_commit(
        &pool,
        &AgentRunCommitUpsert {
            run_id: projected.run.id,
            commit_id: foreign_commits[0].id,
            worktree_id: Some(topology.worktrees[0].id),
            evidence_kind: "git_head_overlap".to_string(),
            evidence_source: "cross-repository-test".to_string(),
            trust_level: EvidenceTrustLevel::Correlated,
            confidence: 0.75,
            observed_at: "2026-08-05T12:02:00.000Z".to_string(),
            metadata_json: "{}".to_string(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("repositories differ"));
    assert!(
        list_agent_run_commits(&pool, projected.run.id)
            .unwrap()
            .is_empty()
    );
}
