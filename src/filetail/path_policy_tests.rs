use serial_test::serial;

use super::path_policy::validate_file_tail_path;

struct EnvGuard {
    key: &'static str,
    value: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: String) -> Self {
        let guard = Self {
            key,
            value: crate::env::var(key).ok(),
        };
        crate::env::set_test_var(key, value);
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        {
            if let Some(value) = &self.value {
                crate::env::set_test_var(self.key, value);
            } else {
                crate::env::remove_test_var(self.key);
            }
        }
    }
}

#[test]
#[serial]
fn env_allowed_root_allows_file_inside_root() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("app.log");
    std::fs::write(&log_path, "hello\n").unwrap();
    let _guard = EnvGuard::set(
        "CORTEX_FILE_TAIL_ALLOWED_ROOTS",
        temp.path().to_string_lossy().into_owned(),
    );

    validate_file_tail_path(&log_path.to_string_lossy()).unwrap();
}

#[cfg(unix)]
#[test]
#[serial]
fn env_allowed_root_rejects_unreadable_file_before_registry_commit() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("auth.log");
    std::fs::write(&log_path, "hello\n").unwrap();
    std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = EnvGuard::set(
        "CORTEX_FILE_TAIL_ALLOWED_ROOTS",
        temp.path().to_string_lossy().into_owned(),
    );

    let err = validate_file_tail_path(&log_path.to_string_lossy()).unwrap_err();

    assert!(err.to_string().contains("not readable"));
}

#[test]
#[serial]
fn symlink_allowed_root_is_canonicalized() {
    let temp = tempfile::tempdir().unwrap();
    let real_root = temp.path().join("real");
    let link_root = temp.path().join("link");
    std::fs::create_dir(&real_root).unwrap();
    std::os::unix::fs::symlink(&real_root, &link_root).unwrap();
    let log_path = real_root.join("app.log");
    std::fs::write(&log_path, "hello\n").unwrap();
    let _guard = EnvGuard::set(
        "CORTEX_FILE_TAIL_ALLOWED_ROOTS",
        link_root.to_string_lossy().into_owned(),
    );

    validate_file_tail_path(&log_path.to_string_lossy()).unwrap();
}

#[test]
#[serial]
fn sensitive_mount_is_denied_even_when_env_allows_parent() {
    let Ok(temp) = tempfile::tempdir_in("/data") else {
        return;
    };
    let log_path = temp.path().join("app.log");
    std::fs::write(&log_path, "hello\n").unwrap();
    let _guard = EnvGuard::set("CORTEX_FILE_TAIL_ALLOWED_ROOTS", "/".to_string());

    let err = validate_file_tail_path(&log_path.to_string_lossy()).unwrap_err();

    assert!(err.to_string().contains("sensitive cortex mount"));
}
