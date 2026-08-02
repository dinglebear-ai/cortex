//! NUL-safe parsers for stable Git porcelain formats.

use std::fmt;

/// Maximum accepted byte length for one NUL-delimited porcelain field.
pub const MAX_PORCELAIN_FIELD_BYTES: usize = 16 * 1024;
const MAX_PORCELAIN_RECORDS: usize = 4096;

/// Unknown future porcelain field retained without decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PorcelainField {
    pub label: Vec<u8>,
    pub value: Option<Vec<u8>>,
}

/// Parsed record from `git worktree list --porcelain -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub path: Vec<u8>,
    pub head: Option<String>,
    pub branch: Option<Vec<u8>>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub lock_reason: Option<Vec<u8>>,
    pub prunable: bool,
    pub prune_reason: Option<Vec<u8>>,
    pub unknown_fields: Vec<PorcelainField>,
}

/// Stable classification for malformed porcelain input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PorcelainParseErrorKind {
    ExpectedWorktreeFirst,
    EmptyWorktreePath,
    DuplicateField(&'static str),
    EmptyFieldValue(&'static str),
    InvalidHead,
    MissingHead,
    MissingBranchOrDetached,
    ConflictingState,
    BareHasHeadOrBranch,
    MissingPruneReason,
    MissingRecordTerminator,
    UnexpectedRecordSeparator,
    TooManyRecords { max: usize },
    FieldTooLong { actual: usize, max: usize },
}

/// Bounded parse error containing only location and classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PorcelainParseError {
    pub record_index: usize,
    pub field_index: usize,
    pub kind: PorcelainParseErrorKind,
}

impl PorcelainParseError {
    fn new(record_index: usize, field_index: usize, kind: PorcelainParseErrorKind) -> Self {
        Self {
            record_index,
            field_index,
            kind,
        }
    }
}

impl fmt::Display for PorcelainParseErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedWorktreeFirst => formatter.write_str("record must start with worktree"),
            Self::EmptyWorktreePath => formatter.write_str("worktree path is empty"),
            Self::DuplicateField(field) => write!(formatter, "duplicate {field} field"),
            Self::EmptyFieldValue(field) => write!(formatter, "{field} value is empty"),
            Self::InvalidHead => formatter.write_str("HEAD must be a 40- or 64-byte hex object ID"),
            Self::MissingHead => formatter.write_str("non-bare worktree is missing HEAD"),
            Self::MissingBranchOrDetached => {
                formatter.write_str("non-bare worktree needs branch or detached")
            }
            Self::ConflictingState => {
                formatter.write_str("branch and detached are mutually exclusive")
            }
            Self::BareHasHeadOrBranch => {
                formatter.write_str("bare worktree cannot include HEAD, branch, or detached")
            }
            Self::MissingPruneReason => formatter.write_str("prunable field requires a reason"),
            Self::MissingRecordTerminator => {
                formatter.write_str("record is missing the blank NUL terminator")
            }
            Self::UnexpectedRecordSeparator => formatter.write_str("unexpected empty record"),
            Self::TooManyRecords { max } => write!(formatter, "record count exceeds {max}"),
            Self::FieldTooLong { actual, max } => {
                write!(formatter, "field is {actual} bytes; maximum is {max}")
            }
        }
    }
}

impl fmt::Display for PorcelainParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "worktree porcelain record {} field {}: {}",
            self.record_index, self.field_index, self.kind
        )
    }
}

impl std::error::Error for PorcelainParseError {}

#[derive(Default)]
struct RecordBuilder {
    path: Vec<u8>,
    head: Option<String>,
    branch: Option<Vec<u8>>,
    detached: bool,
    bare: bool,
    locked: bool,
    lock_reason: Option<Vec<u8>>,
    prunable: bool,
    prune_reason: Option<Vec<u8>>,
    unknown_fields: Vec<PorcelainField>,
}

impl RecordBuilder {
    fn new(path: &[u8]) -> Self {
        Self {
            path: path.to_vec(),
            ..Self::default()
        }
    }

    fn finish(
        self,
        record_index: usize,
        field_index: usize,
    ) -> Result<WorktreeRecord, PorcelainParseError> {
        if self.bare {
            if self.head.is_some() || self.branch.is_some() || self.detached {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::BareHasHeadOrBranch,
                ));
            }
        } else {
            if self.head.is_none() {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::MissingHead,
                ));
            }
            if self.branch.is_some() && self.detached {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::ConflictingState,
                ));
            }
            if self.branch.is_none() && !self.detached {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::MissingBranchOrDetached,
                ));
            }
        }
        if self.prunable && self.prune_reason.is_none() {
            return Err(PorcelainParseError::new(
                record_index,
                field_index,
                PorcelainParseErrorKind::MissingPruneReason,
            ));
        }

        Ok(WorktreeRecord {
            path: self.path,
            head: self.head,
            branch: self.branch,
            detached: self.detached,
            bare: self.bare,
            locked: self.locked,
            lock_reason: self.lock_reason,
            prunable: self.prunable,
            prune_reason: self.prune_reason,
            unknown_fields: self.unknown_fields,
        })
    }
}

fn split_field(field: &[u8]) -> (&[u8], Option<&[u8]>) {
    field
        .iter()
        .position(|byte| *byte == b' ')
        .map_or((field, None), |index| {
            (&field[..index], Some(&field[index + 1..]))
        })
}

fn duplicate(record_index: usize, field_index: usize, name: &'static str) -> PorcelainParseError {
    PorcelainParseError::new(
        record_index,
        field_index,
        PorcelainParseErrorKind::DuplicateField(name),
    )
}

fn required_value<'a>(
    value: Option<&'a [u8]>,
    record_index: usize,
    field_index: usize,
    name: &'static str,
) -> Result<&'a [u8], PorcelainParseError> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(PorcelainParseError::new(
            record_index,
            field_index,
            PorcelainParseErrorKind::EmptyFieldValue(name),
        )),
    }
}

fn parse_head(value: &[u8]) -> Option<String> {
    ((value.len() == 40 || value.len() == 64) && value.iter().all(u8::is_ascii_hexdigit))
        .then(|| String::from_utf8(value.to_vec()).expect("hex object ID is ASCII"))
}

fn parse_record_field(
    builder: &mut RecordBuilder,
    field: &[u8],
    record_index: usize,
    field_index: usize,
) -> Result<(), PorcelainParseError> {
    let (label, value) = split_field(field);
    match label {
        b"worktree" => Err(duplicate(record_index, field_index, "worktree")),
        b"HEAD" => {
            if builder.head.is_some() {
                return Err(duplicate(record_index, field_index, "HEAD"));
            }
            let value = required_value(value, record_index, field_index, "HEAD")?;
            builder.head = parse_head(value);
            if builder.head.is_none() {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::InvalidHead,
                ));
            }
            Ok(())
        }
        b"branch" => {
            if builder.branch.is_some() {
                return Err(duplicate(record_index, field_index, "branch"));
            }
            builder.branch =
                Some(required_value(value, record_index, field_index, "branch")?.to_vec());
            Ok(())
        }
        b"detached" if value.is_none() => {
            if builder.detached {
                return Err(duplicate(record_index, field_index, "detached"));
            }
            builder.detached = true;
            Ok(())
        }
        b"bare" if value.is_none() => {
            if builder.bare {
                return Err(duplicate(record_index, field_index, "bare"));
            }
            builder.bare = true;
            Ok(())
        }
        b"locked" => {
            if builder.locked {
                return Err(duplicate(record_index, field_index, "locked"));
            }
            builder.locked = true;
            builder.lock_reason = value
                .filter(|reason| !reason.is_empty())
                .map(<[u8]>::to_vec);
            Ok(())
        }
        b"prunable" => {
            if builder.prunable {
                return Err(duplicate(record_index, field_index, "prunable"));
            }
            let Some(reason) = value.filter(|reason| !reason.is_empty()) else {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::MissingPruneReason,
                ));
            };
            builder.prunable = true;
            builder.prune_reason = Some(reason.to_vec());
            Ok(())
        }
        _ => {
            builder.unknown_fields.push(PorcelainField {
                label: label.to_vec(),
                value: value.map(<[u8]>::to_vec),
            });
            Ok(())
        }
    }
}

/// Parse `git worktree list --porcelain -z` output without UTF-8 conversion.
pub fn parse_worktree_porcelain(input: &[u8]) -> Result<Vec<WorktreeRecord>, PorcelainParseError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    let mut builder: Option<RecordBuilder> = None;
    let mut record_index = 0;
    let mut field_index = 0;

    let mut fields = input.split(|byte| *byte == 0).peekable();
    while let Some(field) = fields.next() {
        if field.is_empty() && fields.peek().is_none() {
            break;
        }
        if field.is_empty() {
            let Some(current) = builder.take() else {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::UnexpectedRecordSeparator,
                ));
            };
            records.push(current.finish(record_index, field_index)?);
            if records.len() > MAX_PORCELAIN_RECORDS {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::TooManyRecords {
                        max: MAX_PORCELAIN_RECORDS,
                    },
                ));
            }
            record_index += 1;
            field_index = 0;
            continue;
        }

        if field.len() > MAX_PORCELAIN_FIELD_BYTES {
            return Err(PorcelainParseError::new(
                record_index,
                field_index,
                PorcelainParseErrorKind::FieldTooLong {
                    actual: field.len(),
                    max: MAX_PORCELAIN_FIELD_BYTES,
                },
            ));
        }

        if let Some(current) = builder.as_mut() {
            parse_record_field(current, field, record_index, field_index)?;
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            if path.is_empty() {
                return Err(PorcelainParseError::new(
                    record_index,
                    field_index,
                    PorcelainParseErrorKind::EmptyWorktreePath,
                ));
            }
            builder = Some(RecordBuilder::new(path));
        } else {
            return Err(PorcelainParseError::new(
                record_index,
                field_index,
                PorcelainParseErrorKind::ExpectedWorktreeFirst,
            ));
        }
        field_index += 1;
    }

    if builder.is_some() {
        return Err(PorcelainParseError::new(
            record_index,
            field_index,
            PorcelainParseErrorKind::MissingRecordTerminator,
        ));
    }
    Ok(records)
}

#[cfg(test)]
#[path = "porcelain_tests.rs"]
mod tests;
