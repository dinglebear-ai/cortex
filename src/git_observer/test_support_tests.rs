use super::{GitFixture, git_available};
use crate::git_observer::porcelain::parse_worktree_porcelain;

const ROOT_SHA: &str = "a4600ca60e26420e56b54374401fd23ccd4a208d";
const MAIN_SHA: &str = "3deaf115eb2df48b835b5b706d626640b33230d2";
const FEATURE_SHA: &str = "f6a7405024dfb8c42a20bc675fe9093e0bc767fc";
const RESET_DISCARDED_SHA: &str = "5e5c810eaec0f70f0745db09c1299b2766bb6c81";
const REBASE_ORIGINAL_SHA: &str = "96c48c2090c90ff0997e9cecc686a636240df3fb";
const REBASE_SHA: &str = "ee987310aaf16c3916a9c5d033ecd21dd0d143b5";

fn fixture_or_skip() -> Option<GitFixture> {
    if !git_available() {
        eprintln!("skipping deterministic Git fixture test: git executable is unavailable");
        return None;
    }
    Some(GitFixture::build().expect("deterministic Git fixture should build"))
}

#[test]
fn repeated_fixture_builds_produce_exact_commit_vectors() {
    let Some(first) = fixture_or_skip() else {
        return;
    };
    let second = GitFixture::build().expect("second deterministic fixture should build");

    assert_eq!(first.commits, second.commits);
    assert_eq!(first.commits.root, ROOT_SHA);
    assert_eq!(first.commits.main, MAIN_SHA);
    assert_eq!(first.commits.feature, FEATURE_SHA);
    assert_eq!(first.commits.reset_discarded, RESET_DISCARDED_SHA);
    assert_eq!(first.commits.reset_head, ROOT_SHA);
    assert_eq!(first.commits.rebase_original, REBASE_ORIGINAL_SHA);
    assert_eq!(first.commits.rebased, REBASE_SHA);

    let expected = [
        (ROOT_SHA, "", "fixture root", "2026-01-02T03:04:05Z"),
        (MAIN_SHA, ROOT_SHA, "fixture main", "2026-01-02T03:06:05Z"),
        (
            FEATURE_SHA,
            ROOT_SHA,
            "fixture feature",
            "2026-01-02T03:05:05Z",
        ),
        (
            RESET_DISCARDED_SHA,
            ROOT_SHA,
            "fixture discarded",
            "2026-01-02T03:07:05Z",
        ),
        (
            REBASE_ORIGINAL_SHA,
            ROOT_SHA,
            "fixture rebase original",
            "2026-01-02T03:08:05Z",
        ),
        (
            REBASE_SHA,
            MAIN_SHA,
            "fixture rebase original",
            "2026-01-02T03:08:05Z",
        ),
    ];
    for (sha, parent, subject, timestamp) in expected {
        let metadata = first.commit_metadata(sha).unwrap();
        assert_eq!(metadata.sha, sha);
        assert_eq!(metadata.parents, parent);
        assert_eq!(metadata.subject, subject);
        assert_eq!(metadata.authored_at, timestamp);
        assert_eq!(metadata.committed_at, timestamp);
    }
}

#[test]
fn fixture_topology_covers_link_detach_lock_reset_and_rebase_states() {
    let Some(fixture) = fixture_or_skip() else {
        return;
    };

    assert!(fixture.root().is_absolute());
    assert!(fixture.repository().is_absolute());
    assert!(fixture.linked_worktree().is_absolute());
    assert!(fixture.detached_worktree().is_absolute());
    assert_eq!(fixture.branch_head("main").unwrap(), MAIN_SHA);
    assert_eq!(fixture.branch_head("feature").unwrap(), FEATURE_SHA);
    assert_eq!(fixture.branch_head("reset-state").unwrap(), ROOT_SHA);
    assert_eq!(fixture.branch_head("rebase-state").unwrap(), REBASE_SHA);
    assert!(fixture.is_ancestor(ROOT_SHA, MAIN_SHA).unwrap());
    assert!(fixture.is_ancestor(MAIN_SHA, REBASE_SHA).unwrap());
    assert!(!fixture.is_ancestor(RESET_DISCARDED_SHA, ROOT_SHA).unwrap());

    let bytes = fixture
        .git_bytes(
            fixture.repository(),
            &["worktree", "list", "--porcelain", "-z"],
        )
        .unwrap();
    let records = parse_worktree_porcelain(&bytes).unwrap();
    assert_eq!(records.len(), 3);
    let linked = records
        .iter()
        .find(|record| record.path == fixture.linked_worktree().as_os_str().as_encoded_bytes())
        .expect("linked worktree should be listed");
    assert!(linked.locked);
    assert_eq!(
        linked.lock_reason.as_deref(),
        Some(b"fixture lock".as_slice())
    );
    assert_eq!(
        linked.branch.as_deref(),
        Some(b"refs/heads/feature".as_slice())
    );
    let detached = records
        .iter()
        .find(|record| record.path == fixture.detached_worktree().as_os_str().as_encoded_bytes())
        .expect("detached worktree should be listed");
    assert!(detached.detached);
    assert_eq!(detached.head.as_deref(), Some(ROOT_SHA));
}

#[test]
fn fixture_never_reads_or_writes_global_git_configuration() {
    let Some(fixture) = fixture_or_skip() else {
        return;
    };
    assert!(!fixture.home().join(".gitconfig").exists());
    assert!(!fixture.xdg_config_home().join("git/config").exists());
    let origins = fixture
        .git_text(fixture.repository(), &["config", "--show-origin", "--list"])
        .unwrap();
    assert!(!origins.contains(".gitconfig"));
    assert!(!origins.contains("/etc/gitconfig"));
    assert!(origins.contains("core.hookspath=/dev/null"));
    assert!(origins.contains("commit.gpgsign=false"));
}
