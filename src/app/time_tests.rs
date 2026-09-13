use super::*;
use chrono::TimeZone;

fn fixed_now() -> DateTime<Utc> {
    // 2026-06-15T12:00:00Z
    Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
}

#[test]
fn parse_time_arg_parses_relative_durations_back_from_now() {
    let now = fixed_now();
    assert_eq!(
        parse_time_arg("1h", now).unwrap(),
        "2026-06-15T11:00:00+00:00"
    );
    assert_eq!(
        parse_time_arg("30m", now).unwrap(),
        "2026-06-15T11:30:00+00:00"
    );
    assert_eq!(
        parse_time_arg("2d", now).unwrap(),
        "2026-06-13T12:00:00+00:00"
    );
    assert_eq!(
        parse_time_arg("90s", now).unwrap(),
        "2026-06-15T11:58:30+00:00"
    );
}

#[test]
fn parse_time_arg_rejects_unknown_relative_unit() {
    let err = parse_time_arg("5w", fixed_now()).unwrap_err().to_string();
    assert!(err.contains("time"), "error should mention time: {err}");
}

#[test]
fn parse_time_arg_parses_keywords() {
    let now = fixed_now();
    assert_eq!(
        parse_time_arg("now", now).unwrap(),
        "2026-06-15T12:00:00+00:00"
    );
    assert_eq!(
        parse_time_arg("today", now).unwrap(),
        "2026-06-15T00:00:00+00:00"
    );
    assert_eq!(
        parse_time_arg("yesterday", now).unwrap(),
        "2026-06-14T00:00:00+00:00"
    );
}

#[test]
fn parse_time_arg_parses_absolute_timestamps() {
    let now = fixed_now();
    assert_eq!(
        parse_time_arg("2026-06-01T08:30:00Z", now).unwrap(),
        "2026-06-01T08:30:00+00:00"
    );
    assert_eq!(
        parse_time_arg("2026-06-01", now).unwrap(),
        "2026-06-01T00:00:00+00:00"
    );
    assert_eq!(
        parse_time_arg("2026-06-01 08:30", now).unwrap(),
        "2026-06-01T08:30:00+00:00"
    );
}

#[test]
fn parse_time_arg_rejects_multibyte_trailing_char_without_panicking() {
    let now = fixed_now();
    assert!(parse_time_arg("5€", now).is_err());
    assert!(parse_time_arg("2д", now).is_err());
    assert!(parse_time_arg("30м", now).is_err());
}

#[test]
fn parse_time_arg_rejects_negative_relative_duration() {
    let now = fixed_now();
    assert!(parse_time_arg("-5m", now).is_err());
    assert!(parse_time_arg("-1h", now).is_err());
}

#[test]
fn parse_required_timestamp_accepts_relative_forms() {
    // The whole point of the fix: relative/keyword forms now resolve through the
    // service layer (so MCP/REST callers — not just the CLI — accept `30m`).
    assert!(parse_required_timestamp("30m", "since").is_ok());
    assert!(parse_required_timestamp("2d", "until").is_ok());
    assert!(parse_required_timestamp("yesterday", "since").is_ok());
    assert!(parse_required_timestamp("2026-06-01", "since").is_ok());
}

#[test]
fn parse_optional_timestamp_normalizes_to_utc() {
    let parsed = parse_optional_timestamp(Some("2026-01-01T01:00:00+01:00"), "from")
        .unwrap()
        .unwrap();
    assert_eq!(parsed, "2026-01-01T00:00:00.000Z");
}

#[test]
fn parse_optional_timestamp_accepts_absent_value() {
    assert_eq!(parse_optional_timestamp(None, "from").unwrap(), None);
}

#[test]
fn parse_required_timestamp_reports_field_name_and_expected_format() {
    let err = parse_required_timestamp("not-a-date", "reference_time")
        .expect_err("invalid timestamp should fail");

    assert!(err.to_string().contains("Invalid reference_time"));
    // Genuinely unparseable input is rejected by the shared normalizer, whose
    // message points at the accepted forms (including RFC3339).
    assert!(
        err.to_string().contains("RFC3339"),
        "error should mention accepted formats: {err}"
    );
}

#[test]
fn excessive_relative_times_return_invalid_input() {
    for unit in ["s", "m", "h", "d"] {
        assert!(matches!(
            parse_time_arg(&format!("{}{}", i64::MAX, unit), fixed_now()),
            Err(ServiceError::InvalidInput(_))
        ));
    }
    assert!(parse_time_arg("1000000000000d", fixed_now()).is_err());
}
