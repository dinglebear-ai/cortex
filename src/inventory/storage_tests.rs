use super::*;

#[test]
fn write_json_creates_private_file_and_rejects_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    write_json_private(&path, &serde_json::json!({"ok": true})).unwrap();
    assert!(path.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let target = dir.path().join("target.json");
        let link = dir.path().join("link.json");
        std::fs::write(&target, "{}").unwrap();
        symlink(&target, &link).unwrap();
        assert!(write_json_private(&link, &serde_json::json!({})).is_err());
    }
}

#[test]
fn refresh_lock_prevents_concurrent_acquisition_and_releases_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh.lock");
    let lock = RefreshLock::acquire(&path).unwrap();
    assert!(RefreshLock::acquire(&path).is_err());
    assert!(path.exists());
    drop(lock);
    assert!(path.exists());
    assert!(RefreshLock::acquire(&path).is_ok());
}

#[test]
fn stale_refresh_lock_can_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh.lock");
    std::fs::write(&path, "stale").unwrap();

    #[cfg(unix)]
    {
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(1, 0)).unwrap();
        let lock = RefreshLock::acquire(&path).unwrap();
        assert!(path.exists());
        drop(lock);
    }
}

#[test]
#[ignore = "subprocess fixture"]
fn refresh_lock_exit_fixture() {
    let Some(path) = std::env::var_os("CORTEX_TEST_REFRESH_LOCK") else {
        return;
    };
    let _lock = RefreshLock::acquire(Path::new(&path)).unwrap();
    std::process::exit(0); // Intentionally skip Rust destructors.
}

#[test]
fn refresh_lock_recovers_immediately_after_owner_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh.lock");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "inventory::storage::tests::refresh_lock_exit_fixture",
            "--ignored",
        ])
        .env("CORTEX_TEST_REFRESH_LOCK", &path)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(path.exists(), "fixture acquired the lock before exiting");
    let _lock = RefreshLock::acquire(&path).unwrap();
}
