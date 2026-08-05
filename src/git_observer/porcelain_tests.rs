use super::{
    MAX_PORCELAIN_FIELD_BYTES, PorcelainField, PorcelainParseErrorKind, WorktreeRecord,
    parse_worktree_porcelain,
};

const WORKTREES: &[u8] =
    include_bytes!("../../tests/fixtures/git-worktree-porcelain/worktrees.bin");
const BARE: &[u8] = include_bytes!("../../tests/fixtures/git-worktree-porcelain/bare.bin");
const FIXTURE_HEAD: &str = "978cf284916e1e3f9fab5e62fb31ed56a8b36b0d";

fn assert_normal(record: &WorktreeRecord, path: &[u8], branch: &[u8]) {
    assert_eq!(record.path, path);
    assert_eq!(record.head.as_deref(), Some(FIXTURE_HEAD));
    assert_eq!(record.branch.as_deref(), Some(branch));
    assert!(!record.detached);
    assert!(!record.bare);
}

#[test]
fn checked_in_fixture_parses_normal_detached_locked_and_prunable_records() {
    let records = parse_worktree_porcelain(WORKTREES).unwrap();
    assert_eq!(records.len(), 4);

    assert_normal(
        &records[0],
        b"/tmp/ao023-git-fixture/repo",
        b"refs/heads/main",
    );
    assert!(!records[0].locked);
    assert!(!records[0].prunable);
    assert!(records[0].unknown_fields.is_empty());

    assert_eq!(records[1].path, b"/tmp/ao023-git-fixture/detached");
    assert_eq!(records[1].head.as_deref(), Some(FIXTURE_HEAD));
    assert_eq!(records[1].branch, None);
    assert!(records[1].detached);
    assert!(!records[1].bare);

    assert_normal(
        &records[2],
        b"/tmp/ao023-git-fixture/linked",
        b"refs/heads/feature",
    );
    assert!(records[2].locked);
    assert_eq!(
        records[2].lock_reason.as_deref(),
        Some(b"maintenance window".as_slice())
    );
    assert!(!records[2].prunable);

    assert_normal(
        &records[3],
        b"/tmp/ao023-git-fixture/prunable",
        b"refs/heads/prunable",
    );
    assert!(records[3].prunable);
    assert_eq!(
        records[3].prune_reason.as_deref(),
        Some(b"gitdir file points to non-existent location".as_slice())
    );
}

#[test]
fn checked_in_bare_fixture_parses_without_head_or_branch() {
    let records = parse_worktree_porcelain(BARE).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, b"/tmp/ao023-git-fixture/bare.git");
    assert!(records[0].bare);
    assert_eq!(records[0].head, None);
    assert_eq!(records[0].branch, None);
    assert!(!records[0].detached);
}

#[test]
fn unknown_future_fields_are_retained_in_order() {
    let input = b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0branch refs/heads/main\0future-value alpha beta\0future-flag\0\0";
    let records = parse_worktree_porcelain(input).unwrap();
    assert_eq!(
        records[0].unknown_fields,
        vec![
            PorcelainField {
                label: b"future-value".to_vec(),
                value: Some(b"alpha beta".to_vec()),
            },
            PorcelainField {
                label: b"future-flag".to_vec(),
                value: None,
            },
        ]
    );
}

#[test]
fn paths_branches_reasons_and_unknown_values_are_not_lossily_decoded() {
    let input = b"worktree /tmp/\xffrepo\0HEAD 0123456789012345678901234567890123456789\0branch refs/heads/\xfe\0locked \xfd\0future \xfc\0\0";
    let records = parse_worktree_porcelain(input).unwrap();
    assert_eq!(records[0].path, b"/tmp/\xffrepo");
    assert_eq!(
        records[0].branch.as_deref(),
        Some(b"refs/heads/\xfe".as_slice())
    );
    assert_eq!(records[0].lock_reason.as_deref(), Some(b"\xfd".as_slice()));
    assert_eq!(
        records[0].unknown_fields[0].value.as_deref(),
        Some(b"\xfc".as_slice())
    );
}

#[test]
fn sha256_object_ids_are_accepted() {
    let head = "a".repeat(64);
    let input = format!("worktree /repo\0HEAD {head}\0detached\0\0");
    let records = parse_worktree_porcelain(input.as_bytes()).unwrap();
    assert_eq!(records[0].head.as_deref(), Some(head.as_str()));
}

#[test]
fn empty_input_produces_no_records() {
    assert_eq!(parse_worktree_porcelain(b"").unwrap(), Vec::new());
}

#[test]
fn malformed_records_report_typed_locations() {
    let cases: &[(&[u8], PorcelainParseErrorKind)] = &[
        (
            b"HEAD 0123456789012345678901234567890123456789\0\0",
            PorcelainParseErrorKind::ExpectedWorktreeFirst,
        ),
        (
            b"worktree \0bare\0\0",
            PorcelainParseErrorKind::EmptyWorktreePath,
        ),
        (
            b"worktree /repo\0HEAD nope\0detached\0\0",
            PorcelainParseErrorKind::InvalidHead,
        ),
        (
            b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0HEAD 0123456789012345678901234567890123456789\0detached\0\0",
            PorcelainParseErrorKind::DuplicateField("HEAD"),
        ),
        (
            b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0branch refs/heads/main\0detached\0\0",
            PorcelainParseErrorKind::ConflictingState,
        ),
        (
            b"worktree /repo\0bare\0HEAD 0123456789012345678901234567890123456789\0\0",
            PorcelainParseErrorKind::BareHasHeadOrBranch,
        ),
        (
            b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0\0",
            PorcelainParseErrorKind::MissingBranchOrDetached,
        ),
        (
            b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0branch refs/heads/main\0prunable\0\0",
            PorcelainParseErrorKind::MissingPruneReason,
        ),
        (
            b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0branch refs/heads/main\0",
            PorcelainParseErrorKind::MissingRecordTerminator,
        ),
    ];

    for (input, expected) in cases {
        let error = parse_worktree_porcelain(input).unwrap_err();
        assert_eq!(&error.kind, expected);
        assert_eq!(error.record_index, 0);
        assert!(error.to_string().len() <= 192);
    }
}

#[test]
fn oversized_fields_fail_with_a_bounded_error_message() {
    let mut input = b"worktree ".to_vec();
    input.extend(std::iter::repeat_n(b'x', MAX_PORCELAIN_FIELD_BYTES + 1));
    input.extend_from_slice(b"\0bare\0\0");

    let error = parse_worktree_porcelain(&input).unwrap_err();
    assert_eq!(
        error.kind,
        PorcelainParseErrorKind::FieldTooLong {
            actual: MAX_PORCELAIN_FIELD_BYTES + 10,
            max: MAX_PORCELAIN_FIELD_BYTES,
        }
    );
    assert!(error.to_string().len() <= 192);
    assert!(!error.to_string().contains(&"x".repeat(100)));
}

#[test]
fn duplicate_boolean_and_value_fields_are_rejected() {
    for input in [
        b"worktree /repo\0bare\0bare\0\0".as_slice(),
        b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0detached\0detached\0\0".as_slice(),
        b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0branch refs/heads/main\0locked\0locked reason\0\0".as_slice(),
        b"worktree /repo\0HEAD 0123456789012345678901234567890123456789\0branch refs/heads/main\0prunable first\0prunable second\0\0".as_slice(),
    ] {
        assert!(matches!(
            parse_worktree_porcelain(input).unwrap_err().kind,
            PorcelainParseErrorKind::DuplicateField(_)
        ));
    }
}
