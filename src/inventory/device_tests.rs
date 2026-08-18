use super::*;

struct EnvGuard {
    name: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = crate::env::var(name).ok();
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
fn executable_file(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn parse_meminfo_is_optional_and_non_panicking() {
    let _ = parse_meminfo();
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn collect_builds_device_facts_from_local_command_output() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    executable_file(
        &bin_dir.join("hostname"),
        "#!/bin/sh\nprintf 'devhost\\n'\n",
    );
    executable_file(
        &bin_dir.join("uname"),
        "#!/bin/sh\nprintf 'Linux test 1 x86_64 GNU/Linux\\n'\n",
    );
    executable_file(
        &bin_dir.join("ip"),
        r#"#!/bin/sh
printf '[{"ifname":"lo","addr_info":[{"local":"127.0.0.1"}]},{"ifname":"eth0","addr_info":[{"local":"192.0.2.42"},{"local":"fd00::42"}]}]\n'
"#,
    );
    executable_file(
        &bin_dir.join("ss"),
        r#"#!/bin/sh
cat <<'OUT'
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
tcp   LISTEN 0      128    0.0.0.0:3100    0.0.0.0:*
udp   UNCONN 0      0      [::]:1514       [::]:*
OUT
"#,
    );
    executable_file(
        &bin_dir.join("df"),
        r#"#!/bin/sh
cat <<'OUT'
Filesystem Type 1024-blocks Used Available Capacity Mounted on
/dev/sda1 ext4 1000 250 750 25% /
/dev/sdb1 zfs 2000 500 1500 25% /mnt/data
OUT
"#,
    );
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        crate::env::var("PATH").unwrap_or_default()
    );
    let _path_guard = EnvGuard::set("PATH", path);

    let output = collect(std::time::Duration::from_secs(1)).await;

    assert!(output.warnings.is_empty());
    assert_eq!(output.nodes.len(), 1);
    let node = &output.nodes[0];
    assert_eq!(node.hostname, "devhost");
    assert_eq!(node.ips, vec!["192.0.2.42", "fd00::42"]);
    assert_eq!(node.os.as_deref(), Some("Linux test 1 x86_64 GNU/Linux"));
    assert_eq!(node.listeners.len(), 2);
    assert_eq!(node.listeners[0].protocol, "tcp");
    assert_eq!(node.listeners[0].port, Some(3100));
    assert_eq!(node.listeners[1].protocol, "udp");
    assert_eq!(node.listeners[1].port, Some(1514));
    assert_eq!(node.storage.len(), 2);
    assert_eq!(node.storage[0].mount, "/");
    assert_eq!(node.storage[0].total_bytes, Some(1000 * 1024));
    assert_eq!(node.storage[1].mount, "/mnt/data");
    assert_eq!(output.storage.len(), 2);
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn collect_warns_when_optional_device_commands_are_missing() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    executable_file(&bin_dir.join("hostname"), "#!/bin/sh\nprintf '\\n'\n");
    let _path_guard = EnvGuard::set("PATH", bin_dir.as_os_str());

    let output = collect(std::time::Duration::from_millis(50)).await;

    assert_eq!(output.nodes.len(), 1);
    assert_eq!(output.nodes[0].hostname, "localhost");
    assert!(output.nodes[0].ips.is_empty());
    assert!(output.nodes[0].listeners.is_empty());
    assert!(output.nodes[0].storage.is_empty());
    assert!(
        output
            .warnings
            .iter()
            .any(|warning| warning.contains("ip -j addr unavailable"))
    );
    assert!(
        output
            .warnings
            .iter()
            .any(|warning| warning.contains("ss -lntu unavailable"))
    );
    assert!(
        output
            .warnings
            .iter()
            .any(|warning| warning.contains("df -PT unavailable"))
    );
}
