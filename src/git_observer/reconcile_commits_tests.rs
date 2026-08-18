use super::{ReconcileOptions, ReconcileStage, ReconcileWarningKind, reconcile_one_repository};
use crate::agent_observatory::backfill::{run_agent_backfill_chunk, start_agent_backfill};
use crate::agent_observatory::projector::{
    CommandProjectionOutcome, TranscriptProjectionOutcome, project_command_log,
    project_transcript_log,
};
use crate::config::StorageConfig;
use crate::db::LogEntry;
use crate::db::agent_observatory::{
    EvidenceTrustLevel, RepositoryObservationKind, get_worktree_by_key, list_agent_run_commits,
    list_git_commits,
};
use crate::db::init_pool;
use crate::git_observer::test_support::{GitFixture, git_available};
use std::fs;
use std::time::Duration;

fn options(max_commits_per_transition: usize) -> ReconcileOptions {
    ReconcileOptions {
        hostname: "devhost".to_string(),
        command_timeout: Duration::from_secs(5),
        max_commits_per_transition,
        store_changed_paths: true,
        store_author_name: true,
        store_author_email_hash: false,
    }
}

fn create_commit(fixture: &GitFixture, name: &str, subject: &str) -> String {
    fs::write(
        fixture.repository().join(name),
        format!(
            "{subject}
"
        ),
    )
    .unwrap();
    fixture
        .git_text(fixture.repository(), &["add", name])
        .unwrap();
    fixture
        .git_text(fixture.repository(), &["commit", "-m", subject])
        .unwrap();
    fixture
        .git_text(fixture.repository(), &["rev-parse", "HEAD"])
        .unwrap()
}

fn transcript_log(project: &str) -> LogEntry {
    LogEntry {
        id: 7001,
        timestamp: "2026-08-04T14:00:10.000Z".to_string(),
        hostname: "devhost".to_string(),
        facility: Some("agent".to_string()),
        severity: "info".to_string(),
        app_name: Some("Claude".to_string()),
        process_id: Some("4242".to_string()),
        message: "assistant transcript fixture".to_string(),
        received_at: "2026-08-04T14:00:11.000Z".to_string(),
        source_ip: "agent-ai-transcript://devhost".to_string(),
        ai_tool: Some("Claude".to_string()),
        ai_project: Some(project.to_string()),
        ai_session_id: Some("session-one".to_string()),
        ai_transcript_path: Some("/tmp/session-one.jsonl".to_string()),
        metadata_json: Some(r#"{"role":"assistant"}"#.to_string()),
    }
}

fn command_log(cwd: &str) -> LogEntry {
    LogEntry {
        id: 7002,
        timestamp: "2026-08-04T14:00:20.000Z".to_string(),
        hostname: "devhost".to_string(),
        facility: Some("agent".to_string()),
        severity: "info".to_string(),
        app_name: Some("Claude".to_string()),
        process_id: Some("4242".to_string()),
        message: "git commit".to_string(),
        received_at: "2026-08-04T14:00:21.000Z".to_string(),
        source_ip: "agent-command://devhost/claude/session-one".to_string(),
        ai_tool: Some("Claude".to_string()),
        ai_project: Some(cwd.to_string()),
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
                    "cwd":"{cwd}",
                    "pid":4242,
                    "exit_status":0,
                    "duration_ms":70000,
                    "finished_at":"2026-08-04T14:01:30.000Z",
                    "session_id":"session-one"
                }},
                "content_scrubbed":true
            }}"#
        )),
    }
}

#[tokio::test]
async fn two_commit_fast_forward_imports_exact_commits_once_in_order() {
    if !git_available() {
        eprintln!("skipping fast-forward import test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("fast-forward.db"))).unwrap();
    let initial = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:00:00.000Z",
    )
    .await
    .unwrap();
    let repository = initial.topology.unwrap().repository;
    let first_sha = create_commit(&fixture, "ff-one.txt", "fast forward one");
    let second_sha = create_commit(&fixture, "ff-two.txt", "fast forward two");

    let imported = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:01:00.000Z",
    )
    .await
    .unwrap();
    assert!(imported.warnings.is_empty());
    assert_eq!(
        imported
            .imported_commits
            .iter()
            .map(|row| row.sha.as_str())
            .collect::<Vec<_>>(),
        vec![first_sha.as_str(), second_sha.as_str()]
    );
    assert_eq!(imported.imported_commits[0].subject, "fast forward one");
    assert_eq!(imported.imported_commits[1].subject, "fast forward two");
    assert_eq!(
        imported.imported_commits[0].parent_shas_json,
        format!(r#"["{}"]"#, fixture.commits.main)
    );
    assert_eq!(
        imported.imported_commits[1].parent_shas_json,
        format!(r#"["{first_sha}"]"#)
    );
    let heads = imported
        .inserted_observations
        .iter()
        .filter(|row| row.observation_kind == RepositoryObservationKind::Head)
        .collect::<Vec<_>>();
    assert_eq!(heads.len(), 1);
    assert_eq!(
        heads[0].old_head_sha.as_deref(),
        Some(fixture.commits.main.as_str())
    );
    assert_eq!(heads[0].new_head_sha.as_deref(), Some(second_sha.as_str()));

    let rows = list_git_commits(&pool, repository.id).unwrap();
    assert_eq!(rows, imported.imported_commits);
    let repeated = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:02:00.000Z",
    )
    .await
    .unwrap();
    assert!(repeated.imported_commits.is_empty());
    assert!(repeated.inserted_observations.is_empty());
    assert_eq!(list_git_commits(&pool, repository.id).unwrap(), rows);
}

#[tokio::test]
async fn transcript_command_head_change_links_exact_commits_and_backfill_repairs_history() {
    if !git_available() {
        eprintln!("skipping Agent Observatory Git attribution test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("agent-attribution.db"),
    ))
    .unwrap();
    let initial = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:00:00.000Z",
    )
    .await
    .unwrap();
    assert!(initial.topology.is_some());
    let project = fixture.repository().to_str().unwrap();
    let cwd = fixture.repository().join("src");
    let cwd = cwd.to_str().unwrap();

    let TranscriptProjectionOutcome::Projected(transcript) =
        project_transcript_log(&pool, &transcript_log(project)).unwrap()
    else {
        panic!("transcript fixture should project");
    };
    let CommandProjectionOutcome::Projected(command) =
        project_command_log(&pool, &command_log(cwd)).unwrap()
    else {
        panic!("command fixture should project");
    };
    assert_eq!(transcript.run.id, command.run.id);

    let first_sha = create_commit(&fixture, "agent-one.txt", "agent exact one");
    let second_sha = create_commit(&fixture, "agent-two.txt", "agent exact two");
    let imported = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:02:00.000Z",
    )
    .await
    .unwrap();
    assert_eq!(
        imported
            .imported_commits
            .iter()
            .map(|commit| commit.sha.as_str())
            .collect::<Vec<_>>(),
        vec![first_sha.as_str(), second_sha.as_str()]
    );
    let expected_commit_ids = imported
        .imported_commits
        .iter()
        .map(|commit| commit.id)
        .collect::<std::collections::BTreeSet<_>>();
    let relations = list_agent_run_commits(&pool, command.run.id).unwrap();
    assert_eq!(relations.len(), 2);
    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.commit_id)
            .collect::<std::collections::BTreeSet<_>>(),
        expected_commit_ids
    );
    assert!(relations.iter().all(|relation| {
        relation.evidence_kind == "agent_command_cwd"
            && relation.trust_level == EvidenceTrustLevel::Verified
            && (relation.confidence - 0.98).abs() < f64::EPSILON
    }));

    pool.get()
        .unwrap()
        .execute("DELETE FROM agent_run_commits", [])
        .unwrap();
    assert!(
        list_agent_run_commits(&pool, command.run.id)
            .unwrap()
            .is_empty()
    );
    let backfill = start_agent_backfill(&pool).unwrap();
    let mut repaired = backfill.clone();
    for _ in 0..64 {
        if repaired.progress.done {
            break;
        }
        repaired = run_agent_backfill_chunk(&pool, backfill.job_id, 1).unwrap();
    }
    assert!(repaired.progress.done);
    assert!(repaired.progress.commit_relations_written >= 2);
    assert_eq!(
        list_agent_run_commits(&pool, command.run.id).unwrap().len(),
        2
    );

    fixture
        .git_text(fixture.repository(), &["reset", "--hard", &first_sha])
        .unwrap();
    let rewound = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:03:00.000Z",
    )
    .await
    .unwrap();
    assert!(rewound.warnings.is_empty());
    assert_eq!(
        list_agent_run_commits(&pool, command.run.id).unwrap().len(),
        2
    );
}

#[tokio::test]
async fn transition_cap_warns_without_advancing_head_or_importing_partial_commits() {
    if !git_available() {
        eprintln!("skipping fast-forward cap test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("cap.db"))).unwrap();
    let initial = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T15:00:00.000Z",
    )
    .await
    .unwrap();
    let topology = initial.topology.unwrap();
    let main = topology
        .worktrees
        .iter()
        .find(|row| row.path == fixture.repository().to_str().unwrap())
        .unwrap()
        .clone();
    create_commit(&fixture, "cap-one.txt", "cap one");
    create_commit(&fixture, "cap-two.txt", "cap two");

    let capped = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(1),
        "2026-08-04T15:01:00.000Z",
    )
    .await
    .unwrap();
    assert!(capped.topology.is_none());
    assert!(capped.imported_commits.is_empty());
    assert!(capped.inserted_observations.is_empty());
    assert_eq!(capped.warnings.len(), 1);
    assert_eq!(capped.warnings[0].stage, ReconcileStage::CommitTraversal);
    assert_eq!(
        capped.warnings[0].kind,
        ReconcileWarningKind::CommitLimitReached { limit: 1 }
    );
    let persisted = get_worktree_by_key(&pool, &main.worktree_key)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.head_sha, main.head_sha);
    assert!(
        list_git_commits(&pool, topology.repository.id)
            .unwrap()
            .is_empty()
    );
}
