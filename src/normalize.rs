//! Message normalization for error signature detection.
//!
//! `normalize_template` collapses variable log message parts (timestamps,
//! IPs, IDs, hashes, numbers, JSON values, quoted strings, paths) into fixed
//! placeholders so that repeated error patterns map to a single canonical
//! template and, from that, a stable SHA-256 signature hash.
//!
//! **Design constraints**:
//! - No regex pipeline — pure byte/char scanner for hot-path performance.
//! - Must handle multi-byte UTF-8 without splitting codepoints.
//! - JSON pre-pass is highest priority (before any other replacements).
//!
//! `NORMALIZER_VERSION` must be bumped whenever the output of
//! `normalize_template` changes so that stale rows in `error_signatures` are
//! not confused with new ones.

use sha2::{Digest, Sha256};

/// Bump this whenever `normalize_template`'s output changes for any input.
pub(crate) const NORMALIZER_VERSION: i64 = 1;

/// Normalise a message into a template by replacing variable runs with
/// placeholders.
///
/// Priority (highest first):
/// 1. JSON object/array — replace the whole value with `<json>`
/// 2. RFC 3164 timestamp prefix — replace leading `Mon DD HH:MM:SS ` with `<ts> `
/// 3. UUID (8-4-4-4-12 hex) → `<uuid>`
/// 4. IPv4 / IPv4:port → `<ip>` / `<ip>:<n>`
/// 5. Long hex run (≥ 8 chars) → `<hex>`
/// 6. Quoted string (single or double, ≤ 200 chars) → `<str>`
/// 7. Linux absolute path (`/…/…`) → `<path>`
/// 8. Numeric run → `<n>`
/// 9. Non-ASCII codepoints pass through intact.
pub(crate) fn normalize_template(msg: &str) -> String {
    // --- JSON pre-pass (highest priority) -----------------------------------
    // If the entire message is a JSON object or array, replace it wholesale.
    // If a JSON object/array is embedded within the message, replace just that
    // span. We do a simple brace/bracket counter without a full parser so we
    // stay allocation-light.
    let msg = strip_json_spans(msg);

    let bytes = msg.as_bytes();
    let mut out = String::with_capacity(msg.len());
    let mut i = 0;

    // RFC 3164 timestamp prefix: "Mon DD HH:MM:SS " at position 0.
    // e.g. "Jan  1 00:00:00 " or "Jan 12 13:14:15 "
    if let Some(after_ts) = rfc3164_ts_end(bytes) {
        out.push_str("<ts> ");
        i = after_ts;
    }

    while i < bytes.len() {
        let b = bytes[i];

        // Non-ASCII: copy the whole UTF-8 codepoint intact.
        if !b.is_ascii() {
            let ch = msg[i..].chars().next().expect("char at UTF-8 boundary");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }

        // UUID: 8-4-4-4-12 hex separated by dashes
        if is_hex(b) && looks_like_uuid_at(bytes, i) {
            out.push_str("<uuid>");
            i += 36;
            continue;
        }

        // IPv4 / IPv4:port
        if b.is_ascii_digit()
            && let Some(end) = ipv4_end(bytes, i)
        {
            out.push_str("<ip>");
            i = end;
            if i < bytes.len() && bytes[i] == b':' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    out.push_str(":<n>");
                    i = j;
                }
            }
            continue;
        }

        // Long hex run (>= 8 chars)
        if is_hex(b) {
            let mut j = i;
            while j < bytes.len() && is_hex(bytes[j]) {
                j += 1;
            }
            if j - i >= 8 {
                out.push_str("<hex>");
                i = j;
                continue;
            }
        }

        // Quoted string (double or single, capped at 200 chars to avoid
        // swallowing multi-sentence content).
        if b == b'"' || b == b'\'' {
            let quote = b;
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != quote && j - i <= 201 {
                if bytes[j] == b'\\' {
                    j += 1; // skip escaped char
                }
                j += 1;
            }
            if j < bytes.len() && bytes[j] == quote && j - i <= 201 {
                out.push_str("<str>");
                i = j + 1;
                continue;
            }
        }

        // Linux absolute path: starts with / followed by a word-char
        if b == b'/'
            && i + 1 < bytes.len()
            && (bytes[i + 1].is_ascii_alphanumeric() || bytes[i + 1] == b'_')
        {
            let mut j = i + 1;
            while j < bytes.len() {
                let c = bytes[j];
                if c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'-' | b'.' | b'~') {
                    j += 1;
                } else {
                    break;
                }
            }
            if j > i + 1 {
                out.push_str("<path>");
                i = j;
                continue;
            }
        }

        // Numeric run
        if b.is_ascii_digit() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            out.push_str("<n>");
            i = j;
            continue;
        }

        out.push(b as char);
        i += 1;
    }

    out
}

/// Compute a stable SHA-256 hex digest of a normalized template.
pub(crate) fn signature_hash(template: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(template.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// JSON span replacement

/// Walk `msg` and replace any top-level JSON object `{…}` or array `[…]`
/// spans with `<json>`. Handles nesting via a counter; strings (including
/// escaped quotes) are handled so brace/bracket characters inside strings are
/// not counted.
fn strip_json_spans(msg: &str) -> std::borrow::Cow<'_, str> {
    let bytes = msg.as_bytes();
    // Fast check: if there are no `{` or `[` at all, skip the scan.
    if !bytes.iter().any(|&b| b == b'{' || b == b'[') {
        return std::borrow::Cow::Borrowed(msg);
    }

    let mut result = String::new();
    let mut i = 0;
    let mut any_replaced = false;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'{' || b == b'[' {
            let close = if b == b'{' { b'}' } else { b']' };
            if let Some(end) = find_matching_bracket(bytes, i, b, close) {
                if !any_replaced {
                    // Lazy: only materialise `result` on first replacement.
                    result.push_str(&msg[..i]);
                    any_replaced = true;
                }
                result.push_str("<json>");
                i = end + 1;
                continue;
            }
        }
        if any_replaced {
            if b.is_ascii() {
                result.push(b as char);
                i += 1;
            } else {
                let ch = msg[i..].chars().next().expect("UTF-8 boundary");
                result.push(ch);
                i += ch.len_utf8();
            }
        } else {
            i += if b.is_ascii() {
                1
            } else {
                msg[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
            };
        }
    }

    if any_replaced {
        std::borrow::Cow::Owned(result)
    } else {
        std::borrow::Cow::Borrowed(msg)
    }
}

/// Find the matching closing bracket for a JSON object/array starting at
/// `start`. Handles string escaping, nested braces/brackets. Returns the
/// index of the closing bracket, or `None` if not found.
fn find_matching_bracket(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            // Skip over a JSON string
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// RFC 3164 timestamp prefix

/// Detect a leading RFC 3164 timestamp: `Mon DD HH:MM:SS ` (16 or 17 bytes).
/// Month abbreviations: Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec.
/// Returns the index immediately after the space that follows the timestamp.
fn rfc3164_ts_end(bytes: &[u8]) -> Option<usize> {
    // Minimum: "Jan  1 00:00:00 " = 16 bytes (space-padded single digit day)
    //          "Jan 12 00:00:00 " = 16 bytes
    if bytes.len() < 16 {
        return None;
    }
    // Month: 3 ASCII uppercase-then-lower letters
    let month_ok = bytes[0].is_ascii_uppercase()
        && bytes[1].is_ascii_lowercase()
        && bytes[2].is_ascii_lowercase()
        && MONTHS.contains(&(&bytes[0..3] as &[u8]));
    if !month_ok {
        return None;
    }
    if bytes[3] != b' ' {
        return None;
    }
    // Day: space-padded or zero-padded 1-2 digits
    let (day_start, day_end) = if bytes[4] == b' ' {
        // " D"
        (5, 6)
    } else if bytes[4].is_ascii_digit() {
        // "DD"
        (4, 6)
    } else {
        return None;
    };
    if day_end > bytes.len() || !bytes[day_start..day_end].iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let p = day_end;
    if p >= bytes.len() || bytes[p] != b' ' {
        return None;
    }
    let p = p + 1;
    // HH:MM:SS
    if p + 8 > bytes.len() {
        return None;
    }
    let ts = &bytes[p..p + 8];
    // HH:MM:SS pattern: D D : D D : D D
    if !ts[0].is_ascii_digit()
        || !ts[1].is_ascii_digit()
        || ts[2] != b':'
        || !ts[3].is_ascii_digit()
        || !ts[4].is_ascii_digit()
        || ts[5] != b':'
        || !ts[6].is_ascii_digit()
        || !ts[7].is_ascii_digit()
    {
        return None;
    }
    let p = p + 8;
    // Trailing space
    if p >= bytes.len() || bytes[p] != b' ' {
        return None;
    }
    Some(p + 1)
}

static MONTHS: &[&[u8]] = &[
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

// ---------------------------------------------------------------------------
// Shared helpers

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn looks_like_uuid_at(bytes: &[u8], i: usize) -> bool {
    if i + 36 > bytes.len() {
        return false;
    }
    const DASH_POSITIONS: [usize; 4] = [8, 13, 18, 23];
    bytes[i..i + 36].iter().enumerate().all(|(k, &b)| {
        if DASH_POSITIONS.contains(&k) {
            b == b'-'
        } else {
            b.is_ascii_hexdigit()
        }
    })
}

fn ipv4_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    let mut octets = 0;
    while octets < 4 {
        let octet_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let len = i - octet_start;
        if !(1..=3).contains(&len) {
            return None;
        }
        octets += 1;
        if octets < 4 {
            if i >= bytes.len() || bytes[i] != b'.' {
                return None;
            }
            i += 1;
        }
    }
    Some(i)
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
#[path = "normalize_tests.rs"]
mod tests;
