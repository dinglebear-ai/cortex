use super::*;

fn kernel_row(message: &str, hostname: &str) -> LogRow {
    LogRow {
        app_name: Some("kernel".to_string()),
        message: message.to_string(),
        hostname: hostname.to_string(),
        severity: "crit".to_string(),
        metadata_json: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
    }
}

fn container_die_row(hostname: &str, exit_code: &str) -> LogRow {
    let meta = serde_json::json!({
        "action": "die",
        "container_name": "nginx",
        "exit_code": exit_code,
    })
    .to_string();
    LogRow {
        app_name: Some("dockerd".to_string()),
        message: format!("Container nginx died with exit code {exit_code}"),
        hostname: hostname.to_string(),
        severity: "warning".to_string(),
        metadata_json: Some(meta),
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
    }
}

fn container_die_row_numeric(hostname: &str, exit_code: i64) -> LogRow {
    let meta = serde_json::json!({
        "action": "die",
        "container_name": "nginx",
        "exit_code": exit_code,
    })
    .to_string();
    LogRow {
        app_name: Some("dockerd".to_string()),
        message: format!("Container nginx died with exit code {exit_code}"),
        hostname: hostname.to_string(),
        severity: "warning".to_string(),
        metadata_json: Some(meta),
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
    }
}

fn fail2ban_row(hostname: &str, msg: &str) -> LogRow {
    LogRow {
        app_name: Some("fail2ban".to_string()),
        message: msg.to_string(),
        hostname: hostname.to_string(),
        severity: "notice".to_string(),
        metadata_json: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
    }
}

fn authelia_row(hostname: &str) -> LogRow {
    LogRow {
        app_name: Some("authelia".to_string()),
        message: "second_factor authentication failed for user".to_string(),
        hostname: hostname.to_string(),
        severity: "warning".to_string(),
        metadata_json: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
    }
}

#[test]
fn oom_kill_matches() {
    let rows = vec![
        kernel_row("Out of memory: Killed process 1234 (nginx)", "server1"),
        kernel_row("Some unrelated kernel message", "server1"),
    ];
    let results = evaluate_oom_kill(&rows, "[]");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rule_id, "oom_kill");
    assert_eq!(results[0].severity, "critical");
    assert_eq!(results[0].hostname, "server1");
}

#[test]
fn oom_kill_wrong_app_name() {
    let rows = vec![LogRow {
        app_name: Some("systemd".to_string()),
        message: "Out of memory: Killed process 1234 (nginx)".to_string(),
        hostname: "server1".to_string(),
        severity: "crit".to_string(),
        metadata_json: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
    }];
    let results = evaluate_oom_kill(&rows, "[]");
    assert_eq!(results.len(), 0, "should not match non-kernel app_name");
}

#[test]
fn container_die_nonzero_matches() {
    let rows = vec![
        container_die_row("host1", "1"),
        container_die_row("host1", "0"), // exit 0 should not match
    ];
    let results = evaluate_container_die_nonzero(&rows, "[]");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rule_id, "container_die_nonzero");
}

#[test]
fn container_die_exit_zero_ignored() {
    let rows = vec![container_die_row("host1", "0")];
    let results = evaluate_container_die_nonzero(&rows, "[]");
    assert_eq!(results.len(), 0);
}

#[test]
fn container_die_numeric_exit_code_matches() {
    // exit_code as a JSON number (not string) should still be detected
    let rows = vec![
        container_die_row_numeric("host1", 1),
        container_die_row_numeric("host1", 0), // exit 0 should not match
    ];
    let results = evaluate_container_die_nonzero(&rows, "[]");
    assert_eq!(results.len(), 1, "numeric non-zero exit_code should match");
    assert_eq!(results[0].rule_id, "container_die_nonzero");
}

#[test]
fn fail2ban_ban_matches() {
    let rows = vec![
        fail2ban_row(
            "fw1",
            "2026-01-01 00:00:00,000 fail2ban.actions [1234]: NOTICE  [sshd] Ban 1.2.3.4",
        ),
        fail2ban_row(
            "fw1",
            "2026-01-01 00:00:01,000 fail2ban.actions [1234]: NOTICE  [sshd] Unban 1.2.3.4",
        ),
    ];
    let results = evaluate_fail2ban_ban(&rows, "[]");
    assert_eq!(results.len(), 1, "only 'Ban ' messages should match");
    assert_eq!(results[0].rule_id, "fail2ban_ban");
    assert_eq!(results[0].severity, "notice");
}

#[test]
fn authelia_mfa_fail_matches() {
    let rows = vec![
        authelia_row("authhost"),
        LogRow {
            app_name: Some("authelia".to_string()),
            message: "successful login".to_string(),
            hostname: "authhost".to_string(),
            severity: "info".to_string(),
            metadata_json: None,
            timestamp: "2026-01-01T00:00:00.000Z".to_string(),
        },
    ];
    let results = evaluate_authelia_mfa_fail(&rows, "[]");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rule_id, "authelia_mfa_fail");
}

#[test]
fn authelia_mfa_successful_second_factor_does_not_match() {
    // A successful second_factor event should NOT trigger an alert.
    let rows = vec![LogRow {
        app_name: Some("authelia".to_string()),
        message: "second_factor authentication successful for user".to_string(),
        hostname: "authhost".to_string(),
        severity: "info".to_string(),
        metadata_json: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
    }];
    let results = evaluate_authelia_mfa_fail(&rows, "[]");
    assert_eq!(
        results.len(),
        0,
        "successful second_factor should not match"
    );
}

#[test]
fn escaped_titles_in_rules() {
    let rows = vec![kernel_row(
        "Out of memory: Killed process 1234 <nginx@host>",
        "server1",
    )];
    let results = evaluate_oom_kill(&rows, "[]");
    assert!(
        !results[0].title.contains('@'),
        "@ should be escaped in title"
    );
    assert!(
        !results[0].body.contains('<'),
        "< should be stripped from body"
    );
}

#[test]
fn disk_fill_critical_fires() {
    // 300 MiB free, critical threshold = 512 MiB → critical
    let result = evaluate_disk_fill(
        "nas1",
        300 * 1024 * 1024,
        512 * 1024 * 1024,
        768 * 1024 * 1024,
        "[]",
    );
    assert!(result.is_some());
    let p = result.unwrap();
    assert_eq!(p.rule_id, "disk_fill");
    assert_eq!(p.severity, "critical");
    assert_eq!(p.hostname, "nas1");
    assert!(p.dedup_key.contains("nas1"));
    assert!(p.dedup_key.contains("critical"));
}

#[test]
fn disk_fill_warning_fires() {
    // 600 MiB free: above critical (512), below warn (768) → warning
    let result = evaluate_disk_fill(
        "nas1",
        600 * 1024 * 1024,
        512 * 1024 * 1024,
        768 * 1024 * 1024,
        "[]",
    );
    assert!(result.is_some());
    let p = result.unwrap();
    assert_eq!(p.severity, "warning");
    assert!(p.dedup_key.contains("warning"));
}

#[test]
fn disk_fill_ok_does_not_fire() {
    // 1 GiB free: above both thresholds → no alert
    let result = evaluate_disk_fill(
        "nas1",
        1024 * 1024 * 1024,
        512 * 1024 * 1024,
        768 * 1024 * 1024,
        "[]",
    );
    assert!(result.is_none());
}

#[test]
fn disk_fill_zero_thresholds_do_not_fire() {
    // disabled thresholds: critical=0, warn=0 → no alert
    let result = evaluate_disk_fill("nas1", 0, 0, 0, "[]");
    assert!(result.is_none());
}

#[test]
fn ingest_queue_pressure_fires_on_drops() {
    let result = evaluate_ingest_queue_pressure("devhost", 1, 2, 3, 99, 100, "[]");

    let params = result.expect("queue pressure should fire");
    assert_eq!(params.rule_id, "ingest_queue_pressure");
    assert_eq!(params.severity, "warning");
    assert!(params.body.contains("TCP drops"));
}

#[test]
fn ingest_queue_pressure_ok_does_not_fire() {
    let result = evaluate_ingest_queue_pressure("devhost", 0, 0, 0, 0, 100, "[]");
    assert!(result.is_none());
}

#[test]
fn ingest_silence_fires_when_newest_row_exceeds_threshold() {
    let result = evaluate_ingest_silence("devhost", Some(1800), 900, "[]");
    let params = result.expect("silence should fire");
    assert_eq!(params.rule_id, "ingest_silence");
    assert_eq!(params.severity, "critical");
    assert_eq!(params.dedup_key, "ingest_silence:devhost");
    assert!(params.body.contains("30 minutes"));
}

#[test]
fn ingest_silence_recent_rows_do_not_fire() {
    assert!(evaluate_ingest_silence("devhost", Some(60), 900, "[]").is_none());
}

#[test]
fn ingest_silence_empty_db_does_not_fire() {
    // No rows ever = fresh install, not an outage.
    assert!(evaluate_ingest_silence("devhost", None, 900, "[]").is_none());
}

#[test]
fn ingest_silence_zero_threshold_does_not_fire() {
    assert!(evaluate_ingest_silence("devhost", Some(10_000), 0, "[]").is_none());
}

#[test]
fn heartbeat_silence_builds_once_per_outage_dedup_key() {
    let params = evaluate_heartbeat_silence(
        "syslog_7014e4ed",
        "backuphost",
        "2026-07-14T18:34:36.613Z",
        200_000,
        600,
        "[\"gotify://x\"]",
    );
    assert_eq!(params.rule_id, "heartbeat_silence");
    assert_eq!(params.severity, "critical");
    assert_eq!(params.hostname, "backuphost");
    assert_eq!(
        params.dedup_key, "heartbeat_silence:syslog_7014e4ed:2026-07-14T18:34:36.613Z",
        "host_id plus the stalled heartbeat timestamp key the outage — same outage, same key"
    );
    assert!(params.title.contains("backuphost"));
    assert!(params.body.contains("2026-07-14T18:34:36.613Z"));
    assert!(params.body.contains("threshold: 10 min"));

    // A recovery followed by a new outage produces a different key.
    let next = evaluate_heartbeat_silence(
        "syslog_7014e4ed",
        "backuphost",
        "2026-07-20T00:00:00.000Z",
        900,
        600,
        "[\"gotify://x\"]",
    );
    assert_ne!(params.dedup_key, next.dedup_key);
}

#[test]
fn stream_silence_builds_once_per_outage_dedup_key() {
    let params = evaluate_stream_silence(
        "nashost",
        "agent-docker",
        "2026-07-16T20:00:00.000Z",
        7200,
        3600,
        "[\"gotify://x\"]",
    );
    assert_eq!(params.rule_id, "stream_silence");
    assert_eq!(params.severity, "warning");
    assert_eq!(params.hostname, "nashost");
    assert_eq!(
        params.dedup_key,
        "stream_silence:nashost:agent-docker:2026-07-16T20:00:00.000Z"
    );
    assert!(params.title.contains("agent-docker"));
    assert!(params.title.contains("nashost"));
    assert!(params.body.contains("threshold: 60 min"));
}
