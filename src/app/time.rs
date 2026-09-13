use chrono::{DateTime, Duration, SecondsFormat, TimeZone, Utc};

use super::{ServiceError, ServiceResult};

/// Format a UTC instant in the same `Z`-suffixed shape SQLite stores in
/// `received_at` (and that OTLP ingest writes for `timestamp`), so
/// lexicographic TEXT comparisons line up at boundary instants. Plain
/// `to_rfc3339()` produces the `+00:00` form, which sorts strictly less than
/// `Z` character-by-character and can silently drop equal-instant rows from
/// boundary windows.
pub(crate) fn rfc3339_z(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Normalize a user-supplied time value to an RFC3339 string.
///
/// Accepts relative durations (`1h`, `30m`, `2d`, `90s`), the keywords
/// `now`/`today`/`yesterday`, and absolute timestamps (RFC3339, `YYYY-MM-DD`,
/// `YYYY-MM-DD HH:MM`). `now` is injected for deterministic testing — never
/// read the clock inside this function.
///
/// This is the single source of truth for time-value normalization across every
/// entry point. The CLI calls it during argument parsing; the service layer
/// calls it inside [`parse_required_timestamp`], so MCP and REST callers get the
/// same relative-time support the CLI has (an RFC3339 input passes through
/// unchanged — normalization is idempotent).
pub fn parse_time_arg(input: &str, now: DateTime<Utc>) -> ServiceResult<String> {
    let s = input.trim();
    if s.is_empty() {
        return Err(ServiceError::InvalidInput("empty time value".into()));
    }
    match s.to_ascii_lowercase().as_str() {
        "now" => return Ok(now.to_rfc3339()),
        "today" => return Ok(start_of_day(now, 0).to_rfc3339()),
        "yesterday" => return Ok(start_of_day(now, 1).to_rfc3339()),
        _ => {}
    }
    if let Some(dt) = parse_relative(s, now)? {
        return Ok(dt.to_rfc3339());
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc).to_rfc3339());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(Utc
            .from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .to_rfc3339());
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Ok(Utc.from_utc_datetime(&ndt).to_rfc3339());
    }
    Err(ServiceError::InvalidInput(format!(
        "unrecognized time value '{s}'; use e.g. 1h, 30m, 2d, yesterday, 2026-06-01, or an RFC3339 timestamp"
    )))
}

/// Parse a relative duration (`<int><unit>`). Returns `Ok(None)` when `s` is not
/// a relative form so the caller can try absolute parsing; errors when the
/// numeric prefix parses but the unit is unknown or the value is negative.
fn parse_relative(s: &str, now: DateTime<Utc>) -> ServiceResult<Option<DateTime<Utc>>> {
    // `s` is non-empty (the caller guards empty input). Split on the final
    // *character* boundary, not the final byte, so a multibyte trailing char
    // (e.g. `5€`, `2д`) is rejected cleanly instead of panicking in `split_at`.
    let unit_char = s.chars().last().expect("caller guarantees non-empty input");
    let value = &s[..s.len() - unit_char.len_utf8()];
    if let Ok(n) = value.parse::<i64>() {
        if n < 0 {
            return Err(ServiceError::InvalidInput(
                "time value must not be negative; relative offsets are in the past (e.g. 2d)"
                    .into(),
            ));
        }
        let dur = match unit_char {
            's' => Duration::try_seconds(n),
            'm' => Duration::try_minutes(n),
            'h' => Duration::try_hours(n),
            'd' => Duration::try_days(n),
            _ => {
                return Err(ServiceError::InvalidInput(format!(
                    "unknown time unit '{unit_char}'; use s, m, h, or d (e.g. 90s, 2d)"
                )));
            }
        };
        let out_of_range = || ServiceError::InvalidInput("relative time is out of range".into());
        let dur = dur.ok_or_else(out_of_range)?;
        return now
            .checked_sub_signed(dur)
            .map(Some)
            .ok_or_else(out_of_range);
    }
    Ok(None)
}

/// Midnight UTC, `days_ago` days before `now`.
fn start_of_day(now: DateTime<Utc>, days_ago: i64) -> DateTime<Utc> {
    let d = (now - Duration::days(days_ago)).date_naive();
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
}

pub fn parse_optional_timestamp(
    raw: Option<&str>,
    field_name: &str,
) -> ServiceResult<Option<String>> {
    raw.map(|s| parse_required_timestamp(s, field_name).map(rfc3339_z))
        .transpose()
}

pub(super) fn parse_required_timestamp(
    raw: &str,
    field_name: &str,
) -> ServiceResult<DateTime<Utc>> {
    // Normalize relative/keyword/date forms (`1h`, `30m`, `yesterday`,
    // `2026-06-01`) to RFC3339 first so MCP/REST callers get the same support
    // the CLI already has; an RFC3339 input is passed through unchanged.
    let normalized = parse_time_arg(raw, Utc::now())
        .map_err(|e| ServiceError::InvalidInput(format!("Invalid {field_name} '{raw}': {e}")))?;
    DateTime::parse_from_rfc3339(&normalized)
        .map_err(|e| {
            ServiceError::InvalidInput(format!(
                "Invalid {field_name} '{normalized}': {e}. Expected ISO 8601 / RFC3339 format, e.g. '2025-01-15T00:00:00Z'",
            ))
        })
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
