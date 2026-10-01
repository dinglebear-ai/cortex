use super::*;

#[test]
fn plist_is_deterministic_and_escapes() {
    let paths = ["/a & b/cortex", "/e<.env", "/h", "/o", "/x"].map(Path::new);
    let p = render_plist(paths[0], paths[1], paths[2], paths[3], paths[4]);
    assert_eq!(
        p,
        render_plist(paths[0], paths[1], paths[2], paths[3], paths[4])
    );
    assert!(p.contains("/a &amp; b/cortex"));
    assert!(p.contains("/e&lt;.env"));
    assert!(!p.contains("TOKEN"));
}

#[cfg(unix)]
fn run_script(body: &str, deadline: Duration) -> io::Result<Output> {
    launchctl_command(Path::new("/bin/sh"), 501, &["-c", body], deadline)
}

#[test]
#[cfg(unix)]
fn drains_stdout_and_stderr_before_waiting_for_exit() {
    let out = run_script(
        "dd if=/dev/zero bs=131072 count=1 2>/dev/null; dd if=/dev/zero bs=131072 count=1 2>/dev/null | cat >&2",
        Duration::from_secs(1),
    ).unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 131072);
    assert_eq!(out.stderr.len(), 131072);
}

#[test]
#[cfg(unix)]
fn limits_output_while_the_child_is_running() {
    let error = run_script(
        "exec dd if=/dev/zero bs=131072 count=16 2>/dev/null",
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
}

#[test]
#[cfg(unix)]
fn timeout_kills_and_reaps_the_child() {
    let started = Instant::now();
    let error = run_script("exec sleep 30", Duration::from_millis(50)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
#[cfg(unix)]
fn timeout_covers_pipes_inherited_by_a_descendant() {
    let started = Instant::now();
    let error = run_script("sleep 30 & exit 0", Duration::from_millis(50)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
}
