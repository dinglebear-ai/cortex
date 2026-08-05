use super::*;
use crate::inventory::process::CommandOutput;
use futures_util::future::join_all;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[test]
fn ssh_host_parser_keeps_concrete_hosts_only() {
    let (hosts, warnings) = parse_ssh_hosts_with_warnings(
        r#"
Host *
  IdentityFile ~/.ssh/id_ed25519
Host nashost backuphost
  User root
Host github.com
  HostName ssh.github.com
Host winhost-*
  User jmagar
Host devhost
  User jmagar
"#,
    );

    assert_eq!(hosts, vec!["nashost", "backuphost", "devhost"]);
    assert!(warnings.is_empty());
}

#[test]
fn ssh_args_ignore_newer_config_options_before_loading_config() {
    let args = SshContext::new(SshOptions::for_config(Some(std::path::Path::new(
        "/tmp/ssh_config",
    ))))
    .ssh_args("nashost", "true")
    .unwrap();

    assert_eq!(args[0], "-o");
    assert_eq!(args[1], "IgnoreUnknown=WarnWeakCrypto");
    assert_eq!(args[2], "-F");
    assert_eq!(args[3], "/tmp/ssh_config");
}

#[test]
fn ssh_args_allow_busy_hosts_the_full_bounded_probe_window() {
    let args = SshContext::new(SshOptions::default())
        .ssh_args("devhost", "true")
        .unwrap();

    assert!(args.contains(&"ServerAliveInterval=10".to_string()));
    assert!(args.contains(&"ServerAliveCountMax=3".to_string()));
}

#[test]
fn ssh_args_reject_option_like_hosts_and_use_strict_host_keys_by_default() {
    assert!(
        SshContext::new(SshOptions::default())
            .ssh_args("-oProxyCommand=touch /tmp/pwned", "true")
            .unwrap_err()
            .to_string()
            .contains("unsafe ssh host")
    );

    let args = SshContext::new(
        SshOptions::default()
            .with_known_hosts(Some(std::path::PathBuf::from("/tmp/cortex_known_hosts"))),
    )
    .ssh_args("nashost", "true")
    .unwrap();

    assert!(args.contains(&"StrictHostKeyChecking=yes".to_string()));
    assert!(args.contains(&"UserKnownHostsFile=/tmp/cortex_known_hosts".to_string()));
    assert!(args.contains(&"--".to_string()));
    assert_eq!(args[args.len() - 2], "nashost");
}

#[test]
fn ssh_args_can_opt_into_trust_on_first_use() {
    let args =
        SshContext::new(SshOptions::default().with_host_key_policy(SshHostKeyPolicy::AcceptNew))
            .ssh_args("bootstrap-host", "true")
            .unwrap();

    assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
}

#[tokio::test]
async fn ssh_context_limits_concurrency_and_retries_with_backoff() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let attempts = Arc::new(Mutex::new(BTreeMap::<String, usize>::new()));
    let active_for_runner = Arc::clone(&active);
    let max_active_for_runner = Arc::clone(&max_active);
    let attempts_for_runner = Arc::clone(&attempts);

    let context = SshContext::with_runner_for_test(
        SshOptions::default()
            .with_max_concurrent(2)
            .unwrap()
            .with_retry_attempts(2)
            .unwrap()
            .with_retry_initial_backoff(Duration::from_millis(1))
            .unwrap(),
        move |args, _timeout, _max_output_bytes| {
            let active = Arc::clone(&active_for_runner);
            let max_active = Arc::clone(&max_active_for_runner);
            let attempts = Arc::clone(&attempts_for_runner);
            Box::pin(async move {
                let host = args[args.len() - 2].clone();
                let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now_active, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                active.fetch_sub(1, Ordering::SeqCst);

                let mut attempts = attempts.lock().unwrap();
                let attempt = attempts.entry(host).or_default();
                *attempt += 1;
                if *attempt == 1 {
                    anyhow::bail!("transient ssh failure");
                }
                Ok(CommandOutput {
                    status: Some(0),
                    stdout: "ok".to_string(),
                    stderr: String::new(),
                    elapsed_ms: 1,
                    truncated: false,
                })
            })
        },
    );

    let futures = ["a", "b", "c", "d"]
        .into_iter()
        .map(|host| context.run(host, "true", Duration::from_secs(1)));
    let results = join_all(futures).await;

    assert!(results.iter().all(Result::is_ok));
    assert_eq!(max_active.load(Ordering::SeqCst), 2);
    assert_eq!(
        attempts
            .lock()
            .unwrap()
            .values()
            .copied()
            .collect::<Vec<_>>(),
        vec![2, 2, 2, 2]
    );
}

#[test]
fn configured_hosts_reports_rejected_explicit_hosts() {
    let resolution = configured_hosts(
        None,
        &[
            "nashost".to_string(),
            "-oProxyCommand=bad".to_string(),
            "bad host".to_string(),
        ],
    );

    assert_eq!(resolution.hosts, vec!["nashost"]);
    assert!(resolution.explicit_hosts_configured);
    assert_eq!(resolution.warnings.len(), 2);
    assert!(
        resolution
            .warnings
            .iter()
            .any(|warning| warning.contains("-oProxyCommand=bad"))
    );
}

#[test]
fn configured_hosts_reports_all_explicit_hosts_rejected() {
    let resolution = configured_hosts(None, &["-bad".to_string()]);

    assert!(resolution.hosts.is_empty());
    assert!(resolution.no_usable_explicit_hosts());
    assert!(
        resolution
            .warnings
            .iter()
            .any(|warning| warning.contains("all explicitly configured SSH hosts were rejected"))
    );
}

#[test]
fn configured_hosts_reports_unreadable_ssh_config() {
    let missing = std::path::Path::new("/tmp/cortex-missing-ssh-config-for-test");
    let resolution = configured_hosts(Some(missing), &[]);

    assert!(resolution.hosts.is_empty());
    assert!(!resolution.explicit_hosts_configured);
    assert!(
        resolution
            .warnings
            .iter()
            .any(|warning| warning.contains("could not be read"))
    );
}

#[test]
fn ssh_options_reject_invalid_zero_values() {
    assert!(SshOptions::default().with_connect_timeout_secs(0).is_err());
    assert!(SshOptions::default().with_server_alive(0, 1).is_err());
    assert!(SshOptions::default().with_server_alive(1, 0).is_err());
    assert!(SshOptions::default().with_max_concurrent(0).is_err());
    assert!(SshOptions::default().with_retry_attempts(0).is_err());
    assert!(
        SshOptions::default()
            .with_retry_initial_backoff(Duration::ZERO)
            .is_err()
    );

    let args = SshContext::new(SshOptions::default().with_connect_timeout_secs(7).unwrap())
        .ssh_args("nashost", "true")
        .unwrap();
    assert!(args.contains(&"ConnectTimeout=7".to_string()));
    assert!(!args.contains(&"ConnectTimeout=0".to_string()));
}

#[test]
fn retry_jitter_differs_by_host_and_is_bounded() {
    let initial = Duration::from_millis(100);
    let a = backoff_delay_for_host("nashost", initial, 1);
    let b = backoff_delay_for_host("edgehost", initial, 1);
    let base = backoff_delay(initial, 1);

    assert_ne!(a, b, "host-keyed jitter should desynchronize retries");
    assert!(a >= base && a <= base + Duration::from_millis(50));
    assert!(b >= base && b <= base + Duration::from_millis(50));
}

#[tokio::test]
async fn acquire_owned_cancellable_returns_promptly_when_cancelled() {
    let context = SshContext::new(
        SshOptions::default()
            .with_max_concurrent(1)
            .expect("valid limiter"),
    );
    let _held = context.acquire_owned().await.unwrap();
    let token = CancellationToken::new();
    let child = token.child_token();
    let waiter = tokio::spawn({
        let context = context.clone();
        async move { context.acquire_owned_cancellable(&child).await }
    });

    token.cancel();
    let result = tokio::time::timeout(Duration::from_millis(100), waiter)
        .await
        .expect("cancelled acquire should not wait for permit")
        .expect("join");
    assert!(result.unwrap().is_none());
}
