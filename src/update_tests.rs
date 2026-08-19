use super::*;
use serial_test::serial;

const SERVER_HOST: &str = "nashost";
const SERVER_HOME: &str = "/mnt/cache/appdata/cortex";
const CLIENT_HOSTS: &[&str] = &["devhost", "backuphost"];

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

    fn remove(name: &'static str) -> Self {
        let previous = crate::env::var_os(name);
        crate::env::remove_test_var(name);
        Self { name, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => crate::env::set_test_var(self.name, value),
            None => crate::env::remove_test_var(self.name),
        }
    }
}

fn profile_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("deployments.toml")
}

fn client_hosts() -> Vec<String> {
    CLIENT_HOSTS
        .iter()
        .map(|host| (*host).to_string())
        .collect()
}

fn configure_test_server_profile(path: &Path) {
    configure_server_profile(Some(path), SERVER_HOST, SERVER_HOME).unwrap();
}

fn configure_test_clients_profile(path: &Path, hosts: Vec<String>) {
    configure_clients_profile(
        Some(path),
        hosts,
        Some("https://cortex.example.invalid".to_string()),
        Some(true),
        None,
    )
    .unwrap();
}

fn run_test_update(
    scope: UpdateScope,
    path: PathBuf,
    dry_run: bool,
    runner: &mut dyn UpdateRunner,
) -> io::Result<UpdateReport> {
    run_update_with_runner(
        scope,
        UpdateOptions {
            dry_run,
            profile_path: Some(path),
            binary: Some(PathBuf::from("/tmp/cortex")),
        },
        runner,
    )
}

#[test]
fn update_profile_round_trips_server_and_clients() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    let profile = UpdateProfile {
        server: Some(ServerUpdateProfile {
            host: SERVER_HOST.to_string(),
            home: SERVER_HOME.to_string(),
        }),
        clients: ClientsUpdateProfile {
            hosts: client_hosts(),
            target: Some("https://cortex.example.invalid".to_string()),
            docker: Some(true),
            journald: None,
        },
    };

    write_profile(&path, &profile).unwrap();
    let loaded = load_profile(&path).unwrap();

    assert_eq!(loaded, profile);
}

#[test]
fn configure_server_profile_validates_and_preserves_clients() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    write_profile(
        &path,
        &UpdateProfile {
            server: None,
            clients: ClientsUpdateProfile {
                hosts: vec!["devhost".to_string()],
                target: Some("https://cortex.example.invalid".to_string()),
                docker: Some(true),
                journald: Some(false),
            },
        },
    )
    .unwrap();

    let updated = configure_server_profile(Some(&path), SERVER_HOST, SERVER_HOME).unwrap();

    let server = updated.server.as_ref().unwrap();
    assert_eq!(server.host, SERVER_HOST);
    assert_eq!(server.home, SERVER_HOME);
    assert_eq!(updated.clients.hosts, vec!["devhost"]);
}

#[test]
fn configure_server_profile_rejects_unsafe_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);

    let bad_host =
        configure_server_profile(Some(&path), "-oProxyCommand=touch /tmp/pwned", SERVER_HOME)
            .unwrap_err();
    assert!(bad_host.to_string().contains("unsafe ssh host"));

    let bad_home = configure_server_profile(Some(&path), SERVER_HOST, "relative/path").unwrap_err();
    assert!(bad_home.to_string().contains("absolute path"));

    let parent_home =
        configure_server_profile(Some(&path), SERVER_HOST, "/mnt/cache/../cortex").unwrap_err();
    assert!(parent_home.to_string().contains("must not contain '..'"));
}

#[test]
fn configure_clients_profile_merges_omitted_options() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_clients_profile(
        Some(&path),
        vec!["devhost".to_string()],
        Some("https://cortex.example.invalid".to_string()),
        Some(true),
        Some(false),
    )
    .unwrap();

    let updated = configure_clients_profile(
        Some(&path),
        vec!["backuphost".to_string()],
        None,
        None,
        None,
    )
    .unwrap();

    assert_eq!(updated.clients.hosts, vec!["backuphost"]);
    assert_eq!(
        updated.clients.target.as_deref(),
        Some("https://cortex.example.invalid")
    );
    assert_eq!(updated.clients.docker, Some(true));
    assert_eq!(updated.clients.journald, Some(false));
}

#[test]
fn configure_clients_profile_rejects_invalid_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);

    let error = configure_clients_profile(
        Some(&path),
        vec!["devhost".to_string()],
        Some("not a url".to_string()),
        None,
        None,
    )
    .unwrap_err();

    assert!(error.to_string().contains("client target"));
}

#[derive(Default)]
struct FakeUpdateRunner {
    server_calls: Vec<(String, RemoteDeployOptions)>,
    client_calls: Vec<String>,
    probe_calls: Vec<Vec<String>>,
    probes: Vec<crate::agent_deploy::HostProbe>,
    fail_server: bool,
    fail_client: Option<String>,
}

impl UpdateRunner for FakeUpdateRunner {
    fn run_server(
        &mut self,
        host: &str,
        options: RemoteDeployOptions,
    ) -> io::Result<RemoteDeployReport> {
        let home = options.home.clone().unwrap();
        let dry_run = options.dry_run;
        self.server_calls.push((host.to_string(), options));
        Ok(RemoteDeployReport {
            mode: if dry_run { "remote dry-run" } else { "remote" },
            host: host.to_string(),
            env_path: format!("{home}/.env"),
            compose_dir: format!("{home}/compose"),
            data_dir: format!("{home}/data"),
            home,
            health_url: "http://127.0.0.1:3100/health".to_string(),
            mcp_url: "http://127.0.0.1:3100/mcp".to_string(),
            phases: Vec::new(),
            has_errors: self.fail_server,
            elapsed_ms: 1,
        })
    }

    fn deploy_client(
        &mut self,
        host: &str,
        _binary: &Path,
        _config: &AgentDeployConfig,
    ) -> DeployResult {
        self.client_calls.push(host.to_string());
        let ok = self.fail_client.as_deref() != Some(host);
        DeployResult {
            host: host.to_string(),
            ok,
            detail: if ok {
                "ok".to_string()
            } else {
                "forced failure".to_string()
            },
            elapsed_ms: 1,
        }
    }

    fn find_binary(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/tmp/cortex"))
    }

    fn probe_clients(&mut self, hosts: Vec<String>) -> Vec<crate::agent_deploy::HostProbe> {
        self.probe_calls.push(hosts.clone());
        if self.probes.is_empty() {
            hosts
                .into_iter()
                .map(|host| crate::agent_deploy::HostProbe {
                    host,
                    reachable: true,
                    cortex_version: Some("3.9.1".to_string()),
                    agent_active: Some(true),
                })
                .collect()
        } else {
            self.probes.clone()
        }
    }

    fn validate_binary(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn validate_agent_binary_rejects_missing_explicit_binary() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_test_clients_profile(&path, client_hosts());
    let mut runner = RealUpdateRunner;

    let error = run_update_with_runner(
        UpdateScope::Clients,
        UpdateOptions {
            dry_run: true,
            profile_path: Some(path),
            binary: Some(dir.path().join("missing-cortex")),
        },
        &mut runner,
    )
    .unwrap_err();

    assert!(error.to_string().contains("is not usable"));
}

#[test]
fn update_server_uses_saved_profile_without_repeating_home_arg() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_test_server_profile(&path);
    let mut runner = FakeUpdateRunner::default();

    let report = run_update_with_runner(
        UpdateScope::Server,
        UpdateOptions {
            dry_run: true,
            profile_path: Some(path.clone()),
            binary: None,
        },
        &mut runner,
    )
    .unwrap();

    assert!(!report.has_errors);
    assert_eq!(runner.server_calls.len(), 1);
    assert_eq!(runner.server_calls[0].0, SERVER_HOST);
    assert_eq!(runner.server_calls[0].1.home.as_deref(), Some(SERVER_HOME));
    assert!(runner.server_calls[0].1.dry_run);
    assert_eq!(report.profile_path, path);
}

#[test]
#[serial]
fn update_with_explicit_profile_does_not_resolve_default_profile_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_test_server_profile(&path);
    let mut runner = FakeUpdateRunner::default();
    let _home_guard = EnvGuard::set("HOME", "relative-home");
    let _cortex_home_guard = EnvGuard::remove("CORTEX_HOME");

    let report = run_update_with_runner(
        UpdateScope::Server,
        UpdateOptions {
            dry_run: true,
            profile_path: Some(path.clone()),
            binary: None,
        },
        &mut runner,
    )
    .unwrap();

    assert!(!report.has_errors);
    assert_eq!(report.profile_path, path);
    assert_eq!(runner.server_calls.len(), 1);
}

#[test]
fn update_clients_deploys_every_configured_client() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_test_clients_profile(&path, client_hosts());
    let mut runner = FakeUpdateRunner::default();

    let report = run_test_update(UpdateScope::Clients, path, false, &mut runner).unwrap();

    assert!(!report.has_errors);
    assert_eq!(runner.client_calls, client_hosts());
    assert_eq!(report.clients.len(), 2);
}

#[test]
fn update_clients_marks_report_error_when_a_client_deploy_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_test_clients_profile(&path, client_hosts());
    let mut runner = FakeUpdateRunner {
        fail_client: Some("backuphost".to_string()),
        ..FakeUpdateRunner::default()
    };

    let report = run_test_update(UpdateScope::Clients, path, false, &mut runner).unwrap();

    assert!(report.has_errors);
    assert_eq!(runner.client_calls, client_hosts());
    assert!(report.clients.iter().any(|result| !result.ok));
}

#[test]
fn update_clients_dry_run_probes_clients_without_deploying() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    let hosts = client_hosts();
    configure_test_clients_profile(&path, hosts.clone());
    let mut runner = FakeUpdateRunner::default();

    let report = run_test_update(UpdateScope::Clients, path, true, &mut runner).unwrap();

    assert!(!report.has_errors);
    assert_eq!(runner.probe_calls, vec![hosts]);
    assert!(runner.client_calls.is_empty());
    assert_eq!(report.clients.len(), 2);
    assert!(
        report
            .skipped
            .iter()
            .any(|phase| phase.detail.contains("without deploying"))
    );
}

#[test]
fn update_clients_dry_run_marks_missing_probe_as_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    let hosts = client_hosts();
    configure_test_clients_profile(&path, hosts.clone());
    let mut runner = FakeUpdateRunner {
        probes: vec![crate::agent_deploy::HostProbe {
            host: "devhost".to_string(),
            reachable: true,
            cortex_version: Some("3.9.1".to_string()),
            agent_active: Some(true),
        }],
        ..FakeUpdateRunner::default()
    };

    let report = run_test_update(UpdateScope::Clients, path, true, &mut runner).unwrap();

    assert!(report.has_errors);
    assert_eq!(report.clients.len(), 2);
    assert!(report.clients.iter().any(|result| {
        result.host == "backuphost" && !result.ok && result.detail.contains("timed out")
    }));
}

#[test]
fn update_clients_revalidates_loaded_profile_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    write_profile(
        &path,
        &UpdateProfile {
            server: None,
            clients: ClientsUpdateProfile {
                hosts: vec!["-oProxyCommand=touch /tmp/pwned".to_string()],
                target: None,
                docker: None,
                journald: None,
            },
        },
    )
    .unwrap();
    let mut runner = FakeUpdateRunner::default();

    let error = run_test_update(UpdateScope::Clients, path, false, &mut runner).unwrap_err();

    assert!(error.to_string().contains("unsafe ssh host"));
    assert!(runner.client_calls.is_empty());
    assert!(runner.probe_calls.is_empty());
}

#[test]
fn update_clients_revalidates_loaded_profile_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    write_profile(
        &path,
        &UpdateProfile {
            server: None,
            clients: ClientsUpdateProfile {
                hosts: vec!["devhost".to_string()],
                target: Some("notaurl".to_string()),
                docker: None,
                journald: None,
            },
        },
    )
    .unwrap();
    let mut runner = FakeUpdateRunner::default();

    let error = run_test_update(UpdateScope::Clients, path, false, &mut runner).unwrap_err();

    assert!(error.to_string().contains("client target"));
    assert!(runner.client_calls.is_empty());
    assert!(runner.probe_calls.is_empty());
}

#[test]
fn update_all_stops_before_clients_when_server_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_test_server_profile(&path);
    configure_clients_profile(Some(&path), vec!["devhost".to_string()], None, None, None).unwrap();
    let mut runner = FakeUpdateRunner {
        fail_server: true,
        ..FakeUpdateRunner::default()
    };

    let report = run_test_update(UpdateScope::All, path, false, &mut runner).unwrap();

    assert!(report.has_errors);
    assert!(runner.client_calls.is_empty());
    assert!(report.skipped.iter().any(|phase| phase.name == "clients"));
}

#[test]
fn update_all_runs_server_then_clients_when_server_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = profile_path(&dir);
    configure_test_server_profile(&path);
    configure_test_clients_profile(&path, client_hosts());
    let mut runner = FakeUpdateRunner::default();

    let report = run_test_update(UpdateScope::All, path, false, &mut runner).unwrap();

    assert!(!report.has_errors);
    assert_eq!(runner.server_calls.len(), 1);
    assert_eq!(runner.client_calls, client_hosts());
}
