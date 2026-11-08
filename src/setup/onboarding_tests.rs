use super::*;
use serial_test::serial;
struct Guard(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Guard {
    fn new(home: &Path) -> Self {
        let keys = [
            "HOME",
            "CORTEX_HOME",
            "CORTEX_SERVER_URL",
            "CORTEX_TOKEN",
            "CORTEX_API_TOKEN",
            "CODEX_HOME",
            "CORTEX_HEARTBEAT_TOKEN",
            "NO_AUTH",
            "CORTEX_NO_AUTH",
        ];
        let previous = keys
            .iter()
            .map(|key| (*key, crate::env::var_os(key)))
            .collect();
        for key in keys {
            crate::env::remove_test_var(key);
        }
        crate::env::set_test_var("HOME", home);
        Self(previous)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        for (k, v) in &self.0 {
            match v {
                Some(v) => crate::env::set_test_var(k, v),
                None => crate::env::remove_test_var(k),
            }
        }
    }
}
#[tokio::test]
#[serial]
async fn unattended_first_run_requires_role_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    assert!(run_start(StartOptions::default()).await.is_err());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
#[tokio::test]
#[serial]
async fn client_preview_never_creates_server_state_or_config() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    let report = run_start(StartOptions {
        role: Some(Role::Client),
        server: Some("https://example.test".into()),
        clients: Some(vec!["claude".into()]),
        dry_run: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(report.role, Role::Client);
    assert!(report.dry_run);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
#[tokio::test]
#[serial]
async fn client_cannot_silently_keep_collecting_or_start_optional_sources() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    assert!(
        run_start(StartOptions {
            role: Some(Role::Client),
            capabilities: Some(vec!["transcripts".into()]),
            dry_run: true,
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert!(!temp.path().join(".cortex").exists());
}
#[test]
fn secret_input_rejects_multiline_and_unsafe_modes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("secret");
    std::fs::write(&path, "first\nsecond").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(read_secret(&path).is_err());
    std::fs::write(&path, "fixture-credential\n").unwrap();
    assert_eq!(read_secret(&path).unwrap(), "fixture-credential");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_secret(&path).is_err());
    }
}

#[tokio::test]
#[serial]
async fn invalid_server_settings_fail_before_creating_managed_state() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    let result = run_start(StartOptions {
        role: Some(Role::Server),
        settings: [("CORTEX_RETENTION_DAYS".into(), "invalid-days".into())].into(),
        dry_run: true,
        ..Default::default()
    })
    .await;
    assert!(result.is_err());
    assert!(!temp.path().join(".cortex").exists());
}

#[tokio::test]
#[serial]
async fn retained_agent_credentials_do_not_block_client_after_removal() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    std::fs::create_dir(temp.path().join(".cortex")).unwrap();
    super::super::heartbeat_agent_env::write_private_agent_env(
        &temp.path().join(".cortex/heartbeat-agent.env"),
        &[("CORTEX_HEARTBEAT_TOKEN".into(), "retained".into())].into(),
    )
    .unwrap();
    let report = run_start(StartOptions {
        role: Some(Role::Client),
        server: Some("https://example.test".into()),
        capabilities: Some(vec![]),
        dry_run: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(report.role, Role::Client);
    assert!(!temp.path().join(".cortex/setup.toml").exists());
}

#[tokio::test]
#[serial]
async fn first_onboarding_retains_existing_agent_consent() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    let home = temp.path().join(".cortex");
    super::super::heartbeat_agent_env::write_private_agent_env(
        &home.join("heartbeat-agent.env"),
        &[
            ("CORTEX_AGENT_DOCKER".into(), "true".into()),
            (
                "CORTEX_AGENT_DOCKER_URL".into(),
                "http://127.0.0.1:2375".into(),
            ),
        ]
        .into(),
    )
    .unwrap();
    let options = || StartOptions {
        role: Some(Role::Server),
        dry_run: true,
        ..Default::default()
    };
    let report = run_start(options()).await.unwrap();
    assert!(
        report
            .capabilities
            .iter()
            .any(|c| c.id == "docker" && c.status == super::super::CapabilityStatus::Enabled)
    );
    std::fs::write(
        home.join("setup.toml"),
        "role = \"server\"\nclients = []\ncapabilities = []\n",
    )
    .unwrap();
    let report = run_start(options()).await.unwrap();
    assert!(
        report
            .capabilities
            .iter()
            .any(|c| c.id == "docker" && c.status == super::super::CapabilityStatus::Declined)
    );
}

#[tokio::test]
#[serial]
async fn invalid_source_override_fails_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    for (capability, key, value) in [
        (
            "file_tails",
            "CORTEX_AGENT_FILE_TAILS",
            format!("{}:missing", temp.path().join("missing").display()),
        ),
        (
            "syslog_file",
            "CORTEX_AGENT_SYSLOG_FILE",
            temp.path().join("missing").display().to_string(),
        ),
        (
            "docker",
            "CORTEX_AGENT_DOCKER_URL",
            format!("unix://{}", temp.path().join("missing.sock").display()),
        ),
    ] {
        for dry_run in [true, false] {
            assert!(
                run_start(StartOptions {
                    role: Some(Role::Server),
                    capabilities: Some(vec![capability.into()]),
                    settings: [(key.into(), value.clone())].into(),
                    dry_run,
                    ..Default::default()
                })
                .await
                .is_err()
            );
            assert!(!temp.path().join(".cortex").exists());
        }
    }
}

#[tokio::test]
#[serial]
async fn agent_cannot_configure_unauthenticated_mcp_clients_with_only_ingest_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    let path = temp.path().join("ingest");
    super::super::heartbeat_agent_env::atomic_private_write(&path, b"ingest-only", 0o600).unwrap();
    let result = run_start(StartOptions {
        role: Some(Role::Agent),
        server: Some("https://example.test".into()),
        token_file: Some(path),
        clients: Some(vec!["claude".into()]),
        ..Default::default()
    })
    .await;
    assert!(result.is_err());
    assert!(!temp.path().join(".cortex").exists());
    assert!(!temp.path().join(".claude.json").exists());
}

#[tokio::test]
#[serial]
async fn agent_preview_reuses_private_enrollment_with_explicit_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = Guard::new(temp.path());
    let home = temp.path().join(".cortex");
    super::super::heartbeat_agent_env::write_private_agent_env(
        &home.join("heartbeat-agent.env"),
        &[
            ("CORTEX_HEARTBEAT_TOKEN".into(), "saved-ingest".into()),
            (
                "CORTEX_HEARTBEAT_TARGET".into(),
                "https://existing.test".into(),
            ),
        ]
        .into(),
    )
    .unwrap();
    let report = run_start(StartOptions {
        role: Some(Role::Agent),
        dry_run: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(report.server, "https://existing.test");
    assert!(!home.join("setup.toml").exists());
    let replacement = temp.path().join("replacement");
    super::super::heartbeat_agent_env::atomic_private_write(
        &replacement,
        b"replacement-ingest",
        0o600,
    )
    .unwrap();
    let mut values = BTreeMap::new();
    input::restore_agent_enrollment(&home, &mut values).unwrap();
    values.insert(
        "CORTEX_HEARTBEAT_TOKEN".into(),
        read_secret(&replacement).unwrap(),
    );
    assert_eq!(values["CORTEX_HEARTBEAT_TOKEN"], "replacement-ingest");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            home.join("heartbeat-agent.env"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(
            run_start(StartOptions {
                role: Some(Role::Agent),
                dry_run: true,
                ..Default::default()
            })
            .await
            .is_err()
        );
    }
}

#[test]
fn canonical_no_auth_controls_credentials_and_pending_phases() {
    for (alias, canonical, requires_token) in [
        ("false", "true", false),
        ("true", "false", true),
        ("false", "YES", false),
    ] {
        let mut values = [
            ("NO_AUTH".into(), alias.into()),
            ("CORTEX_NO_AUTH".into(), canonical.into()),
        ]
        .into();
        let clients = vec!["codex".into()];
        assert_eq!(
            input::complete_client_credentials(
                Role::Client,
                &clients,
                false,
                false,
                false,
                &mut values
            )
            .is_err(),
            requires_token
        );
        let phases =
            input::credential_phases(Role::Client, &clients, false, None, &values).unwrap();
        assert_eq!(
            phases.iter().any(|p| p.name == "mcp-credential"),
            requires_token
        );
    }
}
