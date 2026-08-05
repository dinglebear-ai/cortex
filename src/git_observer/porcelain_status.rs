//! Summary parser for `git status --porcelain=v2 --branch -z`.

use super::MAX_PORCELAIN_FIELD_BYTES;
use std::fmt;

/// Status metadata and counts without persisted pathnames.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusSummary {
    pub branch_oid: Option<String>,
    pub branch_head: Option<Vec<u8>>,
    pub detached: bool,
    pub initial: bool,
    pub upstream: Option<Vec<u8>>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub staged_count: u64,
    pub unstaged_count: u64,
    pub untracked_count: u64,
    pub conflicted_count: u64,
    pub tracked_record_count: u64,
    pub rename_or_copy_count: u64,
    pub ignored_count: u64,
    pub unknown_header_count: u64,
}

/// Stable classification for malformed status porcelain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusParseErrorKind {
    MissingTerminator,
    MissingBranchOid,
    MissingBranchHead,
    DuplicateHeader(&'static str),
    InvalidBranchOid,
    InvalidAheadBehind,
    InvalidTrackedRecord,
    InvalidUnmergedRecord,
    InvalidXy,
    EmptyPath,
    MissingRenameSource,
    UnknownRecordType,
    FieldTooLong { actual: usize, max: usize },
}

/// Bounded parse error that never embeds filenames or raw fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusParseError {
    pub field_index: usize,
    pub kind: StatusParseErrorKind,
}

impl StatusParseError {
    fn new(field_index: usize, kind: StatusParseErrorKind) -> Self {
        Self { field_index, kind }
    }
}

impl fmt::Display for StatusParseErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTerminator => formatter.write_str("input is missing its final NUL"),
            Self::MissingBranchOid => formatter.write_str("branch.oid header is missing"),
            Self::MissingBranchHead => formatter.write_str("branch.head header is missing"),
            Self::DuplicateHeader(name) => write!(formatter, "duplicate {name} header"),
            Self::InvalidBranchOid => formatter.write_str("branch.oid is invalid"),
            Self::InvalidAheadBehind => formatter.write_str("branch.ab is invalid"),
            Self::InvalidTrackedRecord => formatter.write_str("tracked record is malformed"),
            Self::InvalidUnmergedRecord => formatter.write_str("unmerged record is malformed"),
            Self::InvalidXy => formatter.write_str("XY status is invalid"),
            Self::EmptyPath => formatter.write_str("record pathname is empty"),
            Self::MissingRenameSource => formatter.write_str("rename source pathname is missing"),
            Self::UnknownRecordType => formatter.write_str("record type is unknown"),
            Self::FieldTooLong { actual, max } => {
                write!(formatter, "field is {actual} bytes; maximum is {max}")
            }
        }
    }
}

impl fmt::Display for StatusParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "status porcelain field {}: {}",
            self.field_index, self.kind
        )
    }
}

impl std::error::Error for StatusParseError {}

#[derive(Default)]
struct HeaderState {
    oid_seen: bool,
    head_seen: bool,
    upstream_seen: bool,
    ab_seen: bool,
}

fn is_object_id(value: &[u8]) -> bool {
    (value.len() == 40 || value.len() == 64) && value.iter().all(u8::is_ascii_hexdigit)
}

fn parse_oid(
    value: &[u8],
    summary: &mut StatusSummary,
    headers: &mut HeaderState,
    field_index: usize,
) -> Result<(), StatusParseError> {
    if headers.oid_seen {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::DuplicateHeader("branch.oid"),
        ));
    }
    headers.oid_seen = true;
    if value == b"(initial)" {
        summary.initial = true;
        return Ok(());
    }
    if !is_object_id(value) {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::InvalidBranchOid,
        ));
    }
    summary.branch_oid = Some(String::from_utf8(value.to_vec()).expect("object ID is ASCII"));
    Ok(())
}

fn parse_head(
    value: &[u8],
    summary: &mut StatusSummary,
    headers: &mut HeaderState,
    field_index: usize,
) -> Result<(), StatusParseError> {
    if headers.head_seen {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::DuplicateHeader("branch.head"),
        ));
    }
    headers.head_seen = true;
    if value == b"(detached)" {
        summary.detached = true;
    } else if value.is_empty() {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::MissingBranchHead,
        ));
    } else {
        summary.branch_head = Some(value.to_vec());
    }
    Ok(())
}

fn parse_upstream(
    value: &[u8],
    summary: &mut StatusSummary,
    headers: &mut HeaderState,
    field_index: usize,
) -> Result<(), StatusParseError> {
    if headers.upstream_seen {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::DuplicateHeader("branch.upstream"),
        ));
    }
    headers.upstream_seen = true;
    if value.is_empty() {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::InvalidAheadBehind,
        ));
    }
    summary.upstream = Some(value.to_vec());
    Ok(())
}

fn parse_signed_count(value: &[u8], prefix: u8) -> Option<u64> {
    value
        .strip_prefix(&[prefix])
        .filter(|digits| !digits.is_empty() && digits.iter().all(u8::is_ascii_digit))
        .and_then(|digits| std::str::from_utf8(digits).ok())
        .and_then(|digits| digits.parse().ok())
}

fn parse_ab(
    value: &[u8],
    summary: &mut StatusSummary,
    headers: &mut HeaderState,
    field_index: usize,
) -> Result<(), StatusParseError> {
    if headers.ab_seen {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::DuplicateHeader("branch.ab"),
        ));
    }
    headers.ab_seen = true;
    let mut parts = value.split(|byte| *byte == b' ');
    let ahead = parts.next().and_then(|part| parse_signed_count(part, b'+'));
    let behind = parts.next().and_then(|part| parse_signed_count(part, b'-'));
    if ahead.is_none() || behind.is_none() || parts.next().is_some() {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::InvalidAheadBehind,
        ));
    }
    summary.ahead = ahead;
    summary.behind = behind;
    Ok(())
}

fn parse_header(
    field: &[u8],
    summary: &mut StatusSummary,
    headers: &mut HeaderState,
    field_index: usize,
) -> Result<(), StatusParseError> {
    if let Some(value) = field.strip_prefix(b"# branch.oid ") {
        parse_oid(value, summary, headers, field_index)
    } else if let Some(value) = field.strip_prefix(b"# branch.head ") {
        parse_head(value, summary, headers, field_index)
    } else if let Some(value) = field.strip_prefix(b"# branch.upstream ") {
        parse_upstream(value, summary, headers, field_index)
    } else if let Some(value) = field.strip_prefix(b"# branch.ab ") {
        parse_ab(value, summary, headers, field_index)
    } else {
        summary.unknown_header_count += 1;
        Ok(())
    }
}

fn split_record(field: &[u8], parts: usize) -> Option<Vec<&[u8]>> {
    let values: Vec<&[u8]> = field.splitn(parts, |byte| *byte == b' ').collect();
    (values.len() == parts && values.iter().all(|value| !value.is_empty())).then_some(values)
}

fn valid_xy_byte(value: u8) -> bool {
    matches!(value, b'.' | b'M' | b'T' | b'A' | b'D' | b'R' | b'C' | b'U')
}

fn count_xy(
    xy: &[u8],
    summary: &mut StatusSummary,
    field_index: usize,
) -> Result<(), StatusParseError> {
    if xy.len() != 2 || !valid_xy_byte(xy[0]) || !valid_xy_byte(xy[1]) {
        return Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::InvalidXy,
        ));
    }
    if xy[0] != b'.' {
        summary.staged_count += 1;
    }
    if xy[1] != b'.' {
        summary.unstaged_count += 1;
    }
    Ok(())
}

fn parse_tracked(
    field: &[u8],
    summary: &mut StatusSummary,
    field_index: usize,
) -> Result<bool, StatusParseError> {
    match field.first() {
        Some(b'1') => {
            let parts = split_record(field, 9).ok_or_else(|| {
                StatusParseError::new(field_index, StatusParseErrorKind::InvalidTrackedRecord)
            })?;
            if parts[0] != b"1" {
                return Err(StatusParseError::new(
                    field_index,
                    StatusParseErrorKind::InvalidTrackedRecord,
                ));
            }
            count_xy(parts[1], summary, field_index)?;
            summary.tracked_record_count += 1;
            Ok(false)
        }
        Some(b'2') => {
            let parts = split_record(field, 10).ok_or_else(|| {
                StatusParseError::new(field_index, StatusParseErrorKind::InvalidTrackedRecord)
            })?;
            if parts[0] != b"2" {
                return Err(StatusParseError::new(
                    field_index,
                    StatusParseErrorKind::InvalidTrackedRecord,
                ));
            }
            count_xy(parts[1], summary, field_index)?;
            summary.tracked_record_count += 1;
            summary.rename_or_copy_count += 1;
            Ok(true)
        }
        Some(b'u') => {
            let parts = split_record(field, 11).ok_or_else(|| {
                StatusParseError::new(field_index, StatusParseErrorKind::InvalidUnmergedRecord)
            })?;
            if parts[0] != b"u" {
                return Err(StatusParseError::new(
                    field_index,
                    StatusParseErrorKind::InvalidUnmergedRecord,
                ));
            }
            count_xy(parts[1], summary, field_index)?;
            summary.tracked_record_count += 1;
            summary.conflicted_count += 1;
            Ok(false)
        }
        _ => Err(StatusParseError::new(
            field_index,
            StatusParseErrorKind::UnknownRecordType,
        )),
    }
}

/// Parse status porcelain into branch metadata and counts only.
pub fn parse_status_porcelain_v2(input: &[u8]) -> Result<StatusSummary, StatusParseError> {
    if input.last() != Some(&0) {
        return Err(StatusParseError::new(
            0,
            StatusParseErrorKind::MissingTerminator,
        ));
    }

    let mut summary = StatusSummary::default();
    let mut headers = HeaderState::default();
    let mut expect_rename_source = false;
    let fields = input[..input.len() - 1].split(|byte| *byte == 0);

    for (field_index, field) in fields.enumerate() {
        if field.len() > MAX_PORCELAIN_FIELD_BYTES {
            return Err(StatusParseError::new(
                field_index,
                StatusParseErrorKind::FieldTooLong {
                    actual: field.len(),
                    max: MAX_PORCELAIN_FIELD_BYTES,
                },
            ));
        }
        if expect_rename_source {
            if field.is_empty() {
                return Err(StatusParseError::new(
                    field_index,
                    StatusParseErrorKind::EmptyPath,
                ));
            }
            expect_rename_source = false;
            continue;
        }
        if field.starts_with(b"# ") {
            parse_header(field, &mut summary, &mut headers, field_index)?;
        } else if let Some(path) = field.strip_prefix(b"? ") {
            if path.is_empty() {
                return Err(StatusParseError::new(
                    field_index,
                    StatusParseErrorKind::EmptyPath,
                ));
            }
            summary.untracked_count += 1;
        } else if let Some(path) = field.strip_prefix(b"! ") {
            if path.is_empty() {
                return Err(StatusParseError::new(
                    field_index,
                    StatusParseErrorKind::EmptyPath,
                ));
            }
            summary.ignored_count += 1;
        } else if field.is_empty() {
            return Err(StatusParseError::new(
                field_index,
                StatusParseErrorKind::UnknownRecordType,
            ));
        } else {
            expect_rename_source = parse_tracked(field, &mut summary, field_index)?;
        }
    }

    if expect_rename_source {
        return Err(StatusParseError::new(
            0,
            StatusParseErrorKind::MissingRenameSource,
        ));
    }
    if !headers.oid_seen {
        return Err(StatusParseError::new(
            0,
            StatusParseErrorKind::MissingBranchOid,
        ));
    }
    if !headers.head_seen {
        return Err(StatusParseError::new(
            0,
            StatusParseErrorKind::MissingBranchHead,
        ));
    }
    Ok(summary)
}

#[cfg(test)]
#[path = "porcelain_status_tests.rs"]
mod tests;
