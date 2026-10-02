//! Target-wide lifecycle ownership spans snapshots, writes and verification.
use super::remote_support::{RemoteOutput, RemoteRunner};
use std::{
    io,
    path::{Path, PathBuf},
};
fn token() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
fn quote(value: &str) -> String {
    super::remote_support::shell_quote(value)
}
pub(crate) struct LocalLifecycleLock {
    home: PathBuf,
    token: String,
}
impl LocalLifecycleLock {
    pub(crate) fn acquire(home: &Path) -> io::Result<Self> {
        match std::fs::symlink_metadata(home) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "managed home must be a real directory, not a symlink",
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.uid() != unsafe { libc::geteuid() } {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "managed home is owned by another user",
                        ));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(home)?;
                }
                #[cfg(not(unix))]
                std::fs::create_dir_all(home)?;
            }
            Err(error) => return Err(error),
        }
        let home = std::fs::canonicalize(home)?;
        let lock = home.join(".lifecycle-lock");
        std::fs::create_dir(&lock).map_err(|e| io::Error::new(e.kind(), "another lifecycle operation owns the target; inspect .lifecycle-lock before retrying"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) =
                std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o700))
            {
                let _ = std::fs::remove_dir(&lock);
                return Err(error);
            }
        }
        let token = token();
        if let Err(error) = std::fs::write(lock.join("token"), &token)
            .and_then(|()| std::fs::write(lock.join("pid"), std::process::id().to_string()))
        {
            let _ = std::fs::remove_file(lock.join("token"));
            let _ = std::fs::remove_file(lock.join("pid"));
            let _ = std::fs::remove_dir(&lock);
            return Err(error);
        }
        Ok(Self { home, token })
    }
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}
impl Drop for LocalLifecycleLock {
    fn drop(&mut self) {
        let lock = self.home.join(".lifecycle-lock");
        if std::fs::read_to_string(lock.join("token")).ok().as_deref() == Some(&self.token) {
            let _ = std::fs::remove_file(lock.join("token"));
            let _ = std::fs::remove_file(lock.join("pid"));
            let _ = std::fs::remove_dir(lock);
        }
    }
}
pub(super) struct LockedRunner<'a> {
    runner: &'a mut dyn RemoteRunner,
    host: String,
    home: String,
    token: Option<String>,
}
impl<'a> LockedRunner<'a> {
    pub(super) fn acquire(
        runner: &'a mut dyn RemoteRunner,
        host: &str,
        home: &str,
        mutate: bool,
    ) -> io::Result<Self> {
        let token = mutate.then(token);
        if let Some(token) = &token {
            let script = format!(
                r#"set -eu
umask 077
home={}
docker info >/dev/null
docker compose version >/dev/null
if docker inspect cortex >/dev/null 2>&1; then
  files=$(docker inspect -f '{{{{ index .Config.Labels "com.docker.compose.project.config_files" }}}}' cortex)
  service=$(docker inspect -f '{{{{ index .Config.Labels "com.docker.compose.service" }}}}' cortex)
  case ",$files," in *",$home/compose/docker-compose.yml,"*) ;; *) echo 'Cortex container belongs to another Compose target' >&2; exit 65;; esac
  test "$service" = cortex || exit 65
fi
if test -e "$home" || test -L "$home"; then
  test -d "$home" && ! test -L "$home" || {{ echo 'managed home must be a real directory' >&2; exit 65; }}
  owner=$(stat -c '%u' "$home" 2>/dev/null || stat -f '%u' "$home")
  test "$owner" = "$(id -u)" || {{ echo 'managed home belongs to another user' >&2; exit 65; }}
else
  parent=$(dirname "$home")
  while ! test -d "$parent"; do parent=$(dirname "$parent"); done
  test -w "$parent" || {{ echo 'managed home parent is not writable' >&2; exit 65; }}
fi
if test -e "$home/.lifecycle-lock" || test -L "$home/.lifecycle-lock"; then
  echo 'another lifecycle operation owns the target' >&2; exit 75
fi
mkdir -p "$home"
mkdir "$home/.lifecycle-lock" || {{ echo 'another lifecycle operation owns the target' >&2; exit 75; }}
printf '%s' {} > "$home/.lifecycle-lock/token"
printf '%s' external-deployment > "$home/.lifecycle-lock/pid""#,
                quote(home),
                quote(token)
            );
            let result = runner.run(host, &script, None)?;
            if !result.status_success {
                return Err(io::Error::other(format!(
                    "deployment lifecycle lock failed: {}",
                    result.stderr.trim()
                )));
            }
        }
        Ok(Self {
            runner,
            host: host.into(),
            home: home.into(),
            token,
        })
    }
}
impl RemoteRunner for LockedRunner<'_> {
    fn latest_stable_version(&mut self) -> io::Result<String> {
        self.runner.latest_stable_version()
    }
    fn run(&mut self, host: &str, script: &str, stdin: Option<&str>) -> io::Result<RemoteOutput> {
        let script = match &self.token {
            Some(token) => format!("export CORTEX_LIFECYCLE_TOKEN={}\n{script}", quote(token)),
            None => script.to_string(),
        };
        self.runner.run(host, &script, stdin)
    }
}
impl Drop for LockedRunner<'_> {
    fn drop(&mut self) {
        if let Some(token) = &self.token {
            let script = format!(
                "set -eu\nhome={}\nif test -f \"$home/.lifecycle-lock/token\" && test \"$(cat \"$home/.lifecycle-lock/token\")\" = {}; then rm -f \"$home/.lifecycle-lock/token\" \"$home/.lifecycle-lock/pid\"; rmdir \"$home/.lifecycle-lock\"; fi",
                quote(&self.home),
                quote(token)
            );
            if !self
                .runner
                .run(&self.host, &script, None)
                .is_ok_and(|output| output.status_success)
            {
                eprintln!(
                    "deployment lock cleanup failed; inspect the target .lifecycle-lock before retrying"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn target_lease_rejects_symlink_home_without_touching_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        assert!(LocalLifecycleLock::acquire(&alias).is_err());
        assert!(!target.join(".lifecycle-lock").exists());
    }
    #[test]
    fn target_lease_rejects_file_home_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("file");
        std::fs::write(&home, b"preserved").unwrap();
        assert!(LocalLifecycleLock::acquire(&home).is_err());
        assert_eq!(std::fs::read(&home).unwrap(), b"preserved");
    }
    #[test]
    fn target_lease_rejects_competing_operation_until_owner_drops() {
        let home = tempfile::tempdir().unwrap();
        let owner = LocalLifecycleLock::acquire(home.path()).unwrap();
        assert!(LocalLifecycleLock::acquire(home.path()).is_err());
        assert!(home.path().join(".lifecycle-lock/token").is_file());
        drop(owner);
        assert!(LocalLifecycleLock::acquire(home.path()).is_ok());
    }
}
