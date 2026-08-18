use super::*;

#[cfg(unix)]
fn write_executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

fn path_with_prepended(dir: &std::path::Path) -> std::ffi::OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = crate::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap()
}

struct EnvGuard {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = crate::env::var_os(name);
        crate::env::set_test_var(name, value);
        Self { name, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => {
                crate::env::set_test_var(self.name, value);
            }
            None => {
                crate::env::remove_test_var(self.name);
            }
        }
    }
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn legacy_ai_systemd_units_absent_phase_reports_stale_units() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\ncase \"$*\" in\n  *is-active*cortex-ai-watch.service*) printf 'active\\n' ;;\n  *is-enabled*cortex-ai-watch.service*) printf 'enabled\\n' ;;\n  *is-active*cortex-ai-index.timer*) printf 'inactive\\n' ;;\n  *is-enabled*cortex-ai-index.timer*) printf 'disabled\\n' ;;\n  *) printf 'inactive\\n' ;;\nesac\nexit 0\n",
    );
    let _path = EnvGuard::set("PATH", path_with_prepended(&bin_dir));

    let phase = legacy_ai_systemd_units_absent_phase();

    assert_eq!(phase.name, "legacy-ai-systemd-units-absent");
    assert_eq!(phase.status, SetupStatus::Error);
    assert!(phase.detail.contains("cortex-ai-watch.service"));
    assert!(phase.detail.contains("systemctl --user disable --now"));
}
