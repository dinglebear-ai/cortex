use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

const MAX_CURSOR_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCursor {
    pub sort: String,
    pub id: i64,
    /// Stable row-id ceiling captured before the first page is read.
    pub high_water: i64,
    /// Database timestamp captured with `high_water`; mutable sort columns may
    /// not advance beyond it on later pages.
    pub as_of: String,
    pub direction: CursorDirection,
    pub filters: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    Invalid,
    FilterMismatch,
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid_cursor",
            Self::FilterMismatch => "cursor_filter_mismatch",
        })
    }
}

impl std::error::Error for CursorError {}

pub fn filter_fingerprint<T: Serialize>(filters: &T) -> Result<String, CursorError> {
    let bytes = serde_json::to_vec(filters).map_err(|_| CursorError::Invalid)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn encode_cursor(cursor: &PageCursor) -> Result<String, CursorError> {
    let json = serde_json::to_vec(cursor).map_err(|_| CursorError::Invalid)?;
    Ok(base64url_encode(&json))
}

pub fn decode_cursor(
    encoded: &str,
    expected_filters: &str,
    expected_direction: CursorDirection,
) -> Result<PageCursor, CursorError> {
    if encoded.is_empty() || encoded.len() > MAX_CURSOR_BYTES {
        return Err(CursorError::Invalid);
    }
    let bytes = base64url_decode(encoded)?;
    let cursor: PageCursor = serde_json::from_slice(&bytes).map_err(|_| CursorError::Invalid)?;
    if cursor.id <= 0
        || cursor.high_water <= 0
        || cursor.sort.is_empty()
        || chrono::DateTime::parse_from_rfc3339(&cursor.as_of).is_err()
        || cursor.direction != expected_direction
    {
        return Err(CursorError::Invalid);
    }
    if cursor.filters != expected_filters {
        return Err(CursorError::FilterMismatch);
    }
    Ok(cursor)
}

fn base64url_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[((value >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(TABLE[(value & 63) as usize] as char);
        }
    }
    output
}

fn base64url_decode(input: &str) -> Result<Vec<u8>, CursorError> {
    if input.len() % 4 == 1 {
        return Err(CursorError::Invalid);
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    for chunk in input.as_bytes().chunks(4) {
        let mut value = 0u32;
        for byte in chunk {
            value = (value << 6) | u32::from(decode_byte(*byte).ok_or(CursorError::Invalid)?);
        }
        value <<= (4 - chunk.len()) * 6;
        output.push((value >> 16) as u8);
        if chunk.len() > 2 {
            output.push((value >> 8) as u8);
        }
        if chunk.len() > 3 {
            output.push(value as u8);
        }
    }
    Ok(output)
}

fn decode_byte(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod tests;
