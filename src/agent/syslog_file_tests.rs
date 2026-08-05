use super::*;

#[test]
fn parses_rfc3164_program_and_pid() {
    let parsed = parse_syslog_line(
        "Jun 11 09:24:12 nashost nginx[123]: request handled",
        "fallback",
    );
    assert_eq!(parsed.hostname, "nashost");
    assert_eq!(parsed.app_name, "nginx");
    assert_eq!(parsed.procid, "123");
    assert_eq!(parsed.message, "request handled");
}

#[test]
fn parses_kernel_style_tag_without_pid() {
    let parsed = parse_syslog_line("Jun 11 09:24:12 backuphost kernel: disk online", "fallback");
    assert_eq!(parsed.hostname, "backuphost");
    assert_eq!(parsed.app_name, "kernel");
    assert_eq!(parsed.procid, "-");
    assert_eq!(parsed.message, "disk online");
}

#[test]
fn falls_back_for_unstructured_lines() {
    let parsed = parse_syslog_line("not actually syslog", "nashost");
    assert_eq!(parsed.hostname, "nashost");
    assert_eq!(parsed.app_name, "syslog-file");
    assert_eq!(parsed.procid, "-");
    assert_eq!(parsed.message, "not actually syslog");
}
