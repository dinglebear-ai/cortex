use super::*;

pub(super) struct ReadLine {
    pub(super) text: String,
    pub(super) oversized: bool,
    pub(super) bytes_consumed: usize,
}

pub(super) enum ReadLineOutcome {
    EndOfFile,
    Line(ReadLine),
    /// A bounded pass ended in the middle of a physical record. The reader is
    /// positioned at that exact byte so the next pass can discard only a
    /// bounded fragment rather than searching unboundedly for a newline.
    LimitReached {
        bytes_consumed: usize,
    },
}

/// Read one newline-delimited record, capping it at `MAX_RECORD_SIZE_BYTES`.
///
/// `hasher` is optional: the ingest/checkpoint path passes `Some(..)` to fold
/// each byte into the source-file digest, while callers that only need the
/// text (e.g. `read_transcript_lines`, the skill-event backfill's historical
/// recovery) pass `None` to skip the SHA-256 work entirely.
pub(super) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    mut hasher: Option<&mut Sha256>,
    physical_limit: Option<usize>,
) -> Result<ReadLineOutcome> {
    let mut line = Vec::new();
    let mut oversized = false;
    let mut consumed = 0usize;
    loop {
        if physical_limit.is_some_and(|limit| consumed >= limit) {
            return Ok(ReadLineOutcome::LimitReached {
                bytes_consumed: consumed,
            });
        }
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && !oversized {
                return Ok(ReadLineOutcome::EndOfFile);
            }
            break;
        }
        let permitted = physical_limit.map_or(available.len(), |limit| {
            available.len().min(limit.saturating_sub(consumed))
        });
        let available = &available[..permitted];
        let newline_pos = available.iter().position(|byte| *byte == b'\n');
        let take_len = newline_pos.map_or(available.len(), |pos| pos + 1);
        if let Some(hasher) = hasher.as_deref_mut() {
            hasher.update(&available[..take_len]);
        }
        if !oversized {
            // Copy one sentinel byte past the inclusive record limit so we can
            // detect oversized records while still accepting exactly-limit rows.
            let remaining = MAX_RECORD_SIZE_BYTES
                .saturating_add(1)
                .saturating_sub(line.len());
            let copy_len = take_len.min(remaining);
            line.extend_from_slice(&available[..copy_len]);
            if line.len() > MAX_RECORD_SIZE_BYTES {
                oversized = true;
                line.clear();
            }
        }
        reader.consume(take_len);
        consumed = consumed.saturating_add(take_len);
        if newline_pos.is_some() {
            break;
        }
    }
    if oversized {
        return Ok(ReadLineOutcome::Line(ReadLine {
            text: String::new(),
            oversized: true,
            bytes_consumed: consumed,
        }));
    }
    let text = String::from_utf8(line).context("transcript record is not valid UTF-8")?;
    Ok(ReadLineOutcome::Line(ReadLine {
        text,
        oversized: false,
        bytes_consumed: consumed,
    }))
}
