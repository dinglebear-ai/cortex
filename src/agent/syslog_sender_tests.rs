use super::*;

#[test]
fn backoff_ms_doubles_until_capped() {
    assert_eq!(backoff_ms(0), 500);
    assert_eq!(backoff_ms(1), 1_000);
    assert_eq!(backoff_ms(6), 30_000);
    assert_eq!(backoff_ms(42), 30_000);
}

#[test]
fn local0_pri_clamps_unknown_severity_to_debug() {
    assert_eq!(local0_pri(0), 128);
    assert_eq!(local0_pri(6), PRI_LOCAL0_INFO);
    assert_eq!(local0_pri(99), 135);
}

#[test]
fn format_rfc5424_replaces_newlines_and_keeps_valid_fields() {
    let line = format_rfc5424(
        PRI_LOCAL0_ERR,
        "2026-06-12T12:00:00.000Z",
        "devhost",
        "compose/service",
        "abcdef123456",
        "first\nsecond",
    );

    assert_eq!(
        line,
        "<131>1 2026-06-12T12:00:00.000Z devhost compose/service abcdef123456 - - first second"
    );
}

#[test]
fn format_rfc5424_replaces_empty_spacey_or_long_structured_fields() {
    let long_app = "x".repeat(49);
    assert!(
        format_rfc5424(PRI_LOCAL0_INFO, "ts", "host", "", "pid", "msg")
            .contains(" cortex-agent pid ")
    );
    assert!(
        format_rfc5424(PRI_LOCAL0_INFO, "ts", "host", "bad app", "pid", "msg")
            .contains(" cortex-agent pid ")
    );
    assert!(
        format_rfc5424(PRI_LOCAL0_INFO, "ts", "host", &long_app, "pid", "msg")
            .contains(" cortex-agent pid ")
    );
    assert!(
        format_rfc5424(PRI_LOCAL0_INFO, "ts", "host", "app", "bad pid", "msg")
            .contains(" app - - - msg")
    );
}
