use super::*;

#[test]
fn collapses_all_equal_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join(".env");
    std::fs::write(
        &env_path,
        "CORTEX_AGENT_AI_TRANSCRIPTS=true\nCORTEX_AGENT_AI_TRANSCRIPTS=true\nCORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\nCORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\n",
    )
    .unwrap();

    let result = check_transcript_forward_env_migration(&env_path, true, true);
    assert!(matches!(result.status, SetupStatus::Ok));
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "CORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\n"
    );

    let replay = check_transcript_forward_env_migration(&env_path, true, true);
    assert!(matches!(replay.status, SetupStatus::Ok));
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "CORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\n"
    );
}

#[test]
fn rejects_conflicts_within_duplicate_key() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join(".env");
    let original = "CORTEX_AGENT_AI_TRANSCRIPTS=true\nCORTEX_AGENT_AI_TRANSCRIPTS=false\n";
    std::fs::write(&env_path, original).unwrap();

    let result = check_transcript_forward_env_migration(&env_path, true, true);
    assert!(matches!(result.status, SetupStatus::Error));
    assert!(result.detail.contains("conflicting duplicate"));
    assert_eq!(std::fs::read_to_string(&env_path).unwrap(), original);
}

#[test]
fn atomic_write_rejects_stale_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let env_path = dir.path().join(".env");
    std::fs::write(&env_path, "operator-change=true\n").unwrap();

    let error = atomic_write_env_file(
        &env_path,
        "CORTEX_AGENT_AI_TRANSCRIPTS=true\n",
        "CORTEX_AGENT_AI_TRANSCRIPT_FORWARD=true\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("changed concurrently"));
    assert_eq!(
        std::fs::read_to_string(&env_path).unwrap(),
        "operator-change=true\n"
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlink_without_detaching_it() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("shared.env");
    let env_path = dir.path().join(".env");
    std::fs::write(&target, "CORTEX_AGENT_AI_TRANSCRIPTS=true\n").unwrap();
    symlink(&target, &env_path).unwrap();

    let result = check_transcript_forward_env_migration(&env_path, true, true);
    assert!(matches!(result.status, SetupStatus::Error));
    assert!(result.detail.contains("symbolic link"));
    assert!(
        std::fs::symlink_metadata(&env_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "CORTEX_AGENT_AI_TRANSCRIPTS=true\n"
    );
}
