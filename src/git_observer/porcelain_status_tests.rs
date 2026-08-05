use super::{StatusParseErrorKind, StatusSummary, parse_status_porcelain_v2};

const CLEAN: &[u8] = include_bytes!("../../tests/fixtures/git-status-porcelain-v2/clean.bin");
const DIRTY: &[u8] = include_bytes!("../../tests/fixtures/git-status-porcelain-v2/dirty.bin");
const RENAME: &[u8] = include_bytes!("../../tests/fixtures/git-status-porcelain-v2/rename.bin");
const CONFLICT: &[u8] = include_bytes!("../../tests/fixtures/git-status-porcelain-v2/conflict.bin");
const DETACHED: &[u8] = include_bytes!("../../tests/fixtures/git-status-porcelain-v2/detached.bin");
const NO_UPSTREAM: &[u8] =
    include_bytes!("../../tests/fixtures/git-status-porcelain-v2/no-upstream.bin");
const DIVERGED: &[u8] = include_bytes!("../../tests/fixtures/git-status-porcelain-v2/diverged.bin");
const ROOT_OID: &str = "d6c47ed7ec6e4cdbd030ede149a1a43f15b134e1";

fn assert_clean(summary: &StatusSummary) {
    assert_eq!(summary.branch_oid.as_deref(), Some(ROOT_OID));
    assert_eq!(summary.branch_head.as_deref(), Some(b"main".as_slice()));
    assert!(!summary.detached);
    assert!(!summary.initial);
    assert_eq!(summary.upstream.as_deref(), Some(b"origin/main".as_slice()));
    assert_eq!(summary.ahead, Some(0));
    assert_eq!(summary.behind, Some(0));
    assert_eq!(summary.staged_count, 0);
    assert_eq!(summary.unstaged_count, 0);
    assert_eq!(summary.untracked_count, 0);
    assert_eq!(summary.conflicted_count, 0);
    assert_eq!(summary.tracked_record_count, 0);
    assert_eq!(summary.rename_or_copy_count, 0);
    assert_eq!(summary.ignored_count, 0);
    assert_eq!(summary.unknown_header_count, 0);
}

#[test]
fn clean_fixture_parses_branch_upstream_and_zero_divergence() {
    assert_clean(&parse_status_porcelain_v2(CLEAN).unwrap());
}

#[test]
fn dirty_fixture_counts_staged_unstaged_and_untracked_without_paths() {
    let summary = parse_status_porcelain_v2(DIRTY).unwrap();
    assert_eq!(summary.staged_count, 1);
    assert_eq!(summary.unstaged_count, 1);
    assert_eq!(summary.untracked_count, 1);
    assert_eq!(summary.conflicted_count, 0);
    assert_eq!(summary.tracked_record_count, 2);
    assert_eq!(summary.rename_or_copy_count, 0);
    assert_eq!(summary.branch_head.as_deref(), Some(b"main".as_slice()));
}

#[test]
fn rename_fixture_consumes_orig_path_and_counts_one_staged_record() {
    let summary = parse_status_porcelain_v2(RENAME).unwrap();
    assert_eq!(summary.staged_count, 1);
    assert_eq!(summary.unstaged_count, 0);
    assert_eq!(summary.tracked_record_count, 1);
    assert_eq!(summary.rename_or_copy_count, 1);
    assert_eq!(summary.untracked_count, 0);
}

#[test]
fn conflict_fixture_counts_both_index_and_worktree_sides() {
    let summary = parse_status_porcelain_v2(CONFLICT).unwrap();
    assert_eq!(summary.staged_count, 1);
    assert_eq!(summary.unstaged_count, 1);
    assert_eq!(summary.conflicted_count, 1);
    assert_eq!(summary.tracked_record_count, 1);
    assert_eq!(summary.ahead, Some(1));
    assert_eq!(summary.behind, Some(0));
}

#[test]
fn detached_and_no_upstream_fixtures_preserve_absence() {
    let detached = parse_status_porcelain_v2(DETACHED).unwrap();
    assert_eq!(detached.branch_oid.as_deref(), Some(ROOT_OID));
    assert_eq!(detached.branch_head, None);
    assert!(detached.detached);
    assert_eq!(detached.upstream, None);
    assert_eq!(detached.ahead, None);
    assert_eq!(detached.behind, None);

    let local = parse_status_porcelain_v2(NO_UPSTREAM).unwrap();
    assert_eq!(local.branch_head.as_deref(), Some(b"local-only".as_slice()));
    assert!(!local.detached);
    assert_eq!(local.upstream, None);
    assert_eq!(local.ahead, None);
    assert_eq!(local.behind, None);
}

#[test]
fn diverged_fixture_parses_nonzero_ahead_and_behind() {
    let summary = parse_status_porcelain_v2(DIVERGED).unwrap();
    assert_eq!(summary.branch_head.as_deref(), Some(b"main".as_slice()));
    assert_eq!(summary.upstream.as_deref(), Some(b"origin/main".as_slice()));
    assert_eq!(summary.ahead, Some(1));
    assert_eq!(summary.behind, Some(1));
}

#[test]
fn initial_branch_and_unknown_headers_are_supported() {
    let input = b"# branch.oid (initial)\0# branch.head main\0# future.header raw bytes\0";
    let summary = parse_status_porcelain_v2(input).unwrap();
    assert_eq!(summary.branch_oid, None);
    assert_eq!(summary.branch_head.as_deref(), Some(b"main".as_slice()));
    assert!(summary.initial);
    assert_eq!(summary.unknown_header_count, 1);
}

#[test]
fn non_utf8_filenames_are_counted_but_never_returned() {
    let input = b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\0? \xffname\0! \xfename\0";
    let summary = parse_status_porcelain_v2(input).unwrap();
    assert_eq!(summary.untracked_count, 1);
    assert_eq!(summary.ignored_count, 1);
    let StatusSummary {
        branch_oid: _,
        branch_head: _,
        detached: _,
        initial: _,
        upstream: _,
        ahead: _,
        behind: _,
        staged_count: _,
        unstaged_count: _,
        untracked_count: _,
        conflicted_count: _,
        tracked_record_count: _,
        rename_or_copy_count: _,
        ignored_count: _,
        unknown_header_count: _,
    } = summary;
}

#[test]
fn malformed_headers_and_records_return_typed_bounded_errors() {
    let cases: &[(&[u8], StatusParseErrorKind)] = &[
        (b"# branch.head main\0", StatusParseErrorKind::MissingBranchOid),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0",
            StatusParseErrorKind::MissingBranchHead,
        ),
        (
            b"# branch.oid nope\0# branch.head main\0",
            StatusParseErrorKind::InvalidBranchOid,
        ),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\0",
            StatusParseErrorKind::DuplicateHeader("branch.oid"),
        ),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\0# branch.ab +x -1\0",
            StatusParseErrorKind::InvalidAheadBehind,
        ),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\x002 R. N... 100644 100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 R100 new\0",
            StatusParseErrorKind::MissingRenameSource,
        ),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\0? \0",
            StatusParseErrorKind::EmptyPath,
        ),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\x001 ZZ N... 100644 100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 file\0",
            StatusParseErrorKind::InvalidXy,
        ),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\0x future\0",
            StatusParseErrorKind::UnknownRecordType,
        ),
        (
            b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main",
            StatusParseErrorKind::MissingTerminator,
        ),
    ];

    for (input, expected) in cases {
        let error = parse_status_porcelain_v2(input).unwrap_err();
        assert_eq!(&error.kind, expected);
        assert!(error.to_string().len() <= 192);
    }
}

#[test]
fn duplicate_upstream_and_ab_headers_are_rejected() {
    let prefix = b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\0";
    let mut upstream = prefix.to_vec();
    upstream.extend_from_slice(b"# branch.upstream origin/main\0# branch.upstream fork/main\0");
    assert_eq!(
        parse_status_porcelain_v2(&upstream).unwrap_err().kind,
        StatusParseErrorKind::DuplicateHeader("branch.upstream")
    );

    let mut ab = prefix.to_vec();
    ab.extend_from_slice(b"# branch.ab +0 -0\0# branch.ab +1 -1\0");
    assert_eq!(
        parse_status_porcelain_v2(&ab).unwrap_err().kind,
        StatusParseErrorKind::DuplicateHeader("branch.ab")
    );
}
