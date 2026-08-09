use super::*;

#[test]
fn fix_fails_closed_and_preserves_equal_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join(".env");
    let original = "CORTEX_AGENT_AI_TRANSCRIPTS=true\nCORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\n";
    std::fs::write(&env_path, original).unwrap();

    let result = check_transcript_forward_env_migration(&env_path, true, true);
    assert!(matches!(result.status, SetupStatus::Error));
    assert!(result.detail.contains("automatic rewrite is disabled"));
    assert_eq!(std::fs::read_to_string(&env_path).unwrap(), original);
}

#[test]
fn noncooperative_edit_after_validated_read_is_never_lost() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join(".env");
    std::fs::write(&env_path, "CORTEX_AGENT_AI_TRANSCRIPTS=true\n").unwrap();

    let result = check_transcript_forward_env_migration_with_hook(&env_path, true, true, || {
        std::fs::write(&env_path, "operator-change=true\n").unwrap();
    });

    assert!(matches!(result.status, SetupStatus::Error));
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "operator-change=true\n"
    );
}

#[cfg(unix)]
#[test]
fn noncooperative_symlink_swap_after_validated_read_is_not_detached() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("shared.env");
    let env_path = dir.path().join(".env");
    std::fs::write(&env_path, "CORTEX_AGENT_AI_TRANSCRIPTS=true\n").unwrap();
    std::fs::write(&target, "operator-target=true\n").unwrap();

    let result = check_transcript_forward_env_migration_with_hook(&env_path, true, true, || {
        std::fs::remove_file(&env_path).unwrap();
        symlink(&target, &env_path).unwrap();
    });

    assert!(matches!(result.status, SetupStatus::Error));
    assert!(
        std::fs::symlink_metadata(&env_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "operator-target=true\n"
    );
}
