use super::*;

#[test]
fn parse_entry_formats_journald_json_as_syslog_line() {
    let line = r#"{
        "MESSAGE": "disk almost full",
        "PRIORITY": "3",
        "SYSLOG_IDENTIFIER": "systemd",
        "_PID": "1234"
    }"#;

    let parsed = parse_entry("devhost", line).unwrap();

    assert!(parsed.starts_with("<131>1 "));
    assert!(parsed.contains(" devhost systemd 1234 - - disk almost full"));
}

#[test]
fn parse_entry_uses_systemd_unit_and_default_priority_when_fields_are_missing() {
    let line = r#"{
        "MESSAGE": "started service",
        "_SYSTEMD_UNIT": "cortex.service"
    }"#;

    let parsed = parse_entry("devhost", line).unwrap();

    assert!(parsed.starts_with("<134>1 "));
    assert!(parsed.contains(" devhost cortex.service - - - started service"));
}

#[test]
fn parse_entry_rejects_invalid_empty_or_messageless_entries() {
    assert_eq!(parse_entry("devhost", "not-json"), None);
    assert_eq!(parse_entry("devhost", r#"{"PRIORITY":"3"}"#), None);
    assert_eq!(parse_entry("devhost", r#"{"MESSAGE":"   "}"#), None);
}

#[cfg(unix)]
#[tokio::test]
async fn journal_exit_is_a_restartable_error_even_when_successful() {
    let dir = tempfile::tempdir().unwrap();
    let sender = Arc::new(SyslogSender::new(
        String::new(),
        None,
        dir.path().join("spool"),
    ));
    for code in [0, 7] {
        let child = tokio::process::Command::new("sh")
            .args(["-c", &format!("exit {code}")])
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let error = forward_child(child, "host", sender.clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("stream ended unexpectedly"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn sender_failure_kills_and_reaps_idle_journal_child() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("not-a-directory");
    std::fs::create_dir(&blocker).unwrap();
    let sender = Arc::new(SyslogSender::new(
        String::new(),
        None,
        blocker.join("spool"),
    ));
    std::fs::remove_dir_all(&blocker).unwrap();
    std::fs::write(&blocker, "blocked").unwrap();
    let child = tokio::process::Command::new("sh")
        .args(["-c", r#"printf '%s\n' '{"MESSAGE":"test"}'; exec sleep 60"#])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            forward_child(child, "host", sender)
        )
        .await
        .unwrap()
        .is_err()
    );
    // SAFETY: signal 0 only queries existence; it does not signal another process.
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_forwarder_terminates_idle_child() {
    let dir = tempfile::tempdir().unwrap();
    let sender = Arc::new(SyslogSender::new(
        String::new(),
        None,
        dir.path().join("spool"),
    ));
    let child = tokio::process::Command::new("sleep")
        .arg("60")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let task = tokio::spawn(forward_child(child, "host", sender));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            // SAFETY: signal zero only checks whether this child still exists.
            if unsafe { libc::kill(pid as i32, 0) } == -1 {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
