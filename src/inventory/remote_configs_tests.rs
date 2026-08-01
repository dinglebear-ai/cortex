use super::*;
use crate::inventory::process::CommandOutput;
use crate::inventory::ssh::SshOptions;

#[test]
fn record_parser_preserves_typed_complete_frames() {
    let records = parse_records(
        "noise\n\u{1e}proxy\t/tmp/a.conf\nserver_name a.test;\u{1f}\n\u{1e}compose\t/tmp/b.yml\nservices:\n  app:\n    image: test\u{1f}\n",
    );

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, RemoteConfigKind::Proxy);
    assert_eq!(records[0].path, "/tmp/a.conf");
    assert_eq!(records[0].body, "server_name a.test;");
    assert_eq!(records[1].kind, RemoteConfigKind::Compose);
    assert_eq!(records[1].path, "/tmp/b.yml");
    assert_eq!(records[1].body, "services:\n  app:\n    image: test");
}

#[test]
fn record_parser_drops_incomplete_trailing_frame() {
    let records = parse_records(
        "\u{1e}compose\t/tmp/ok.yml\nservices: {}\u{1f}\n\u{1e}adguard\t/tmp/config.yaml\ndns:\n  port: 53",
    );

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, "/tmp/ok.yml");
}

#[tokio::test]
async fn remote_records_warns_when_malformed_frames_are_dropped() {
    let context = SshContext::with_runner_for_test(SshOptions::default(), |_, _, _| {
        Box::pin(async {
            Ok(CommandOutput {
                status: Some(0),
                stdout: "\u{1e}compose\t/tmp/ok.yml\nservices: {}\u{1f}\n\u{1e}adguard\t/tmp/config.yaml\ndns:\n  port: 53"
                    .to_string(),
                stderr: String::new(),
                elapsed_ms: 1,
                truncated: false,
            })
        })
    });
    let mut out = CollectorOutput::new("raw_configs");

    let records = remote_records(&mut out, "host", &context, Duration::from_secs(1)).await;

    assert_eq!(records.len(), 1);
    assert!(out.warnings.iter().any(|warning| {
        warning.contains("dropped 1 incomplete or malformed record frame")
            && warning.contains("host")
    }));
}

#[test]
fn config_batch_command_covers_compose_proxy_and_adguard_roots_once() {
    let command = config_batch_command();

    assert!(command.contains("/mnt/compose"));
    assert!(command.contains("/mnt/appdata/swag/nginx/proxy-confs"));
    assert!(command.contains("/mnt/appdata/adguard/etc"));
    assert!(command.contains("AdGuardHome.yaml"));
    assert!(command.contains("\\036compose\\t"));
    assert!(command.contains("\\036proxy\\t"));
    assert!(command.contains("\\036adguard\\t"));
    assert!(command.contains("\\037\\n"));
}

#[tokio::test]
async fn collect_warns_and_skips_when_no_explicit_hosts_are_usable() {
    let dir = tempfile::tempdir().unwrap();
    let paths = InventoryPaths::new(dir.path().join("inventory"));
    let context = SshContext::new(SshOptions::default());

    let out = collect(
        None,
        &["-bad-host".to_string()],
        &context,
        &paths,
        "run-1",
        Duration::from_millis(10),
        Duration::from_millis(20),
    )
    .await;

    assert!(out.artifacts.is_empty());
    assert!(out.compose_projects.is_empty());
    assert!(
        out.warnings
            .iter()
            .any(|warning| warning.contains("no explicitly configured SSH hosts"))
    );
    assert!(
        out.warnings
            .iter()
            .any(|warning| warning.contains("rejected unsafe configured SSH host"))
    );
}

#[tokio::test]
async fn collector_deadline_preserves_completed_host_results() {
    let dir = tempfile::tempdir().unwrap();
    let paths = InventoryPaths::new(dir.path().join("inventory"));
    paths.ensure_private_dirs().unwrap();
    let context = SshContext::with_runner_for_test(
        SshOptions::default().with_max_concurrent(2).unwrap(),
        |args, _timeout, max_output_bytes| {
            Box::pin(async move {
                assert!(max_output_bytes >= MAX_RAW_ARTIFACT_BYTES);
                let host = args[args.len() - 2].clone();
                if host == "slow" {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    anyhow::bail!("slow host should be cancelled by collector deadline");
                }
                Ok(CommandOutput {
                    status: Some(0),
                    stdout: "\u{1e}compose\t/mnt/compose/app/compose.yaml\nservices:\n  app:\n    image: test\u{1f}\n"
                        .to_string(),
                    stderr: String::new(),
                    elapsed_ms: 1,
                    truncated: false,
                })
            })
        },
    );

    let out = collect(
        None,
        &["fast".to_string(), "slow".to_string()],
        &context,
        &paths,
        "run",
        Duration::from_secs(1),
        Duration::from_millis(30),
    )
    .await;

    assert_eq!(out.compose_projects.len(), 1);
    assert_eq!(out.compose_projects[0].services, vec!["app"]);
    assert_eq!(out.artifacts.len(), 1);
    assert!(out.warnings.iter().any(|warning| {
        warning.contains("preserved completed hosts") && warning.contains("slow")
    }));
}

#[test]
fn merge_output_appends_remote_services_artifacts_and_warnings() {
    let mut out = CollectorOutput::new("raw_configs");
    let mut remote = CollectorOutput::new("raw_configs");
    remote.warn("remote_config", "ssh failed");
    remote
        .artifacts
        .push(crate::inventory::schema::ArtifactRef {
            id: "artifact-a".to_string(),
            kind: "compose".to_string(),
            collector: "raw_configs".to_string(),
            source_host: Some("host".to_string()),
            source_path: Some("/tmp/docker-compose.yml".to_string()),
            cache_path: "/tmp/cache".to_string(),
            redaction: crate::inventory::schema::RedactionStatus::NoSecretsDetected,
            byte_len: 12,
            truncated: false,
        });

    merge_output(&mut out, remote);

    assert_eq!(out.artifacts.len(), 1);
    assert_eq!(out.warnings, vec!["ssh failed".to_string()]);
    assert_eq!(out.errors.len(), 1);
}
