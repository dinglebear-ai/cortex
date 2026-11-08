//! Bounded, literal text for every push delivery path.

use serde_json::Value;

const MAX_INPUT_CHARS: usize = 16_384;
const MAX_BODY_CHARS: usize = 1_200;
const MAX_FIELDS: usize = 8;
const MAX_DEPTH: usize = 12;
const DETAILS: &str = "Additional details are available in Cortex.";
const PRIORITY_KEYS: &[&str] = &[
    "message", "msg", "error", "err", "reason", "detail", "details",
];

pub(crate) fn severity_icon(severity: &str) -> &'static str {
    match severity {
        "emerg" | "alert" | "crit" | "critical" | "err" | "error" => "🚨",
        "warning" | "warn" => "⚠️",
        _ => "ℹ️",
    }
}

/// Format JSON as labeled fields rather than exposing wire payloads. This also
/// runs for persisted outbox rows and direct test/health notifications.
pub(crate) fn readable_text(input: &str) -> String {
    let mut input_chars = input.chars();
    let bounded: String = input_chars.by_ref().take(MAX_INPUT_CHARS).collect();
    let was_truncated = input_chars.next().is_some();
    // Older outbox entries can already contain multiple HTML-escape layers.
    // Delivery uses text format, so the characters must remain literal.
    let mut decoded = bounded;
    for _ in 0..6 {
        let next = decoded
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&");
        if next == decoded {
            break;
        }
        decoded = next;
    }
    if decoded.lines().any(|line| line.starts_with("Signature: "))
        && decoded.lines().any(|line| line.starts_with("Sample: "))
    {
        decoded = decoded
            .lines()
            .filter(|line| !line.starts_with("Signature: "))
            .map(|line| {
                if let Some(sample) = line.strip_prefix("Sample: ") {
                    format!("Problem: {}", log_message(sample))
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let formatted = render_text(&decoded, 0);
    let literal: String = formatted
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .map(|c| if c == '@' { '＠' } else { c })
        .collect();
    truncate(literal.trim(), MAX_BODY_CHARS, was_truncated)
}

pub(crate) fn readable_title(input: &str) -> String {
    truncate(&readable_text(input).replace('\n', " · "), 120, false)
}

fn truncate(input: &str, limit: usize, was_truncated: bool) -> String {
    if was_truncated || input.chars().count() > limit {
        let mut out: String = input.chars().take(limit - 1).collect();
        out.push('…');
        out
    } else {
        input.to_string()
    }
}

fn render_text(input: &str, depth: usize) -> String {
    if depth >= MAX_DEPTH {
        return DETAILS.to_string();
    }
    if let Ok(value @ (Value::Object(_) | Value::Array(_) | Value::String(_))) =
        serde_json::from_str::<Value>(input.trim())
    {
        return render_json(&value, depth + 1);
    }
    let normalized = input
        .lines()
        .map(log_message)
        .collect::<Vec<_>>()
        .join("\n");
    render_fragments(&normalized, depth)
}

fn log_message(line: &str) -> &str {
    let mut parts = line.splitn(4, '\t');
    let timestamp = parts.next().unwrap_or_default();
    if timestamp.starts_with(|c: char| c.is_ascii_digit()) && timestamp.contains('T') {
        let _severity = parts.next();
        let _caller = parts.next();
        if let Some(message) = parts.next() {
            return message;
        }
    }
    line
}

fn json_start(input: &str) -> Option<usize> {
    input.char_indices().find_map(|(index, c)| {
        let after = input[index + c.len_utf8()..].trim_start();
        let next = after.chars().next();
        let candidate = match c {
            '{' => matches!(next, None | Some('"' | '}' | '\'')) || after.starts_with("\\\""),
            '[' if after
                .split_once(']')
                .is_some_and(|(address, _)| address.parse::<std::net::Ipv6Addr>().is_ok()) =>
            {
                false
            }
            '[' => {
                matches!(next, None | Some('"' | '{' | '[' | ']' | '-' | '0'..='9'))
                    || after.starts_with("true")
                    || after.starts_with("false")
                    || after.starts_with("null")
                    || after.starts_with("\\\"")
            }
            _ => false,
        };
        candidate.then_some(index)
    })
}

fn render_fragments(input: &str, depth: usize) -> String {
    let mut remainder = input;
    let mut parts = Vec::new();
    while let Some(start) = json_start(remainder) {
        let mut prefix = remainder[..start].trim();
        let mut stream =
            serde_json::Deserializer::from_str(&remainder[start..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                let details = render_json(&value, depth + 1);
                for label in ["Problem:", "Cause:"] {
                    if details.starts_with(label)
                        && let Some(before_label) = prefix.strip_suffix(label)
                    {
                        prefix = before_label.trim_end();
                    }
                }
                if !prefix.is_empty() {
                    parts.push(prefix.to_string());
                }
                parts.push(details);
                remainder = &remainder[start + stream.byte_offset()..];
            }
            _ => {
                // A truncated or normalized payload is not safe to display as
                // raw text. Keep its readable prefix and point to full evidence.
                if !prefix.is_empty() {
                    parts.push(prefix.to_string());
                }
                parts.push(DETAILS.to_string());
                remainder = "";
                break;
            }
        }
    }
    if !remainder.trim().is_empty() {
        parts.push(remainder.trim().replace('\t', " "));
    }
    parts.join("\n")
}

fn render_json(value: &Value, depth: usize) -> String {
    let mut fields = Vec::new();
    append_fields(value, "", depth, &mut fields);
    if fields.is_empty() {
        DETAILS.to_string()
    } else {
        fields.join("\n")
    }
}

fn append_fields(value: &Value, key: &str, depth: usize, fields: &mut Vec<String>) {
    if fields.len() >= MAX_FIELDS || depth >= MAX_DEPTH {
        return;
    }
    match value {
        Value::Object(object) => {
            for name in PRIORITY_KEYS.iter().copied().chain(
                object
                    .keys()
                    .map(String::as_str)
                    .filter(|name| !PRIORITY_KEYS.contains(name)),
            ) {
                if ignored_key(name) {
                    continue;
                }
                if let Some(value) = object.get(name) {
                    append_fields(value, name, depth + 1, fields);
                }
            }
        }
        Value::Array(values) => {
            for value in values.iter().take(MAX_FIELDS) {
                append_fields(value, key, depth + 1, fields);
            }
        }
        Value::Null => {}
        value => {
            let text = match value {
                Value::String(text) => {
                    if let Ok(nested @ (Value::Object(_) | Value::Array(_) | Value::String(_))) =
                        serde_json::from_str::<Value>(text.trim())
                    {
                        append_fields(&nested, key, depth + 1, fields);
                        return;
                    }
                    render_text(text, depth + 1)
                }
                other => other.to_string(),
            };
            if !text.is_empty() {
                let label = field_label(key);
                fields.push(if label.is_empty() {
                    text
                } else {
                    format!("{label}: {text}")
                });
            }
        }
    }
}

fn ignored_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "resource"
            | "scope"
            | "stack"
            | "stacktrace"
            | "stack_trace"
            | "trace_id"
            | "span_id"
            | "service.instance.id"
            | "timestamp"
            | "ts"
            | "time"
            | "level"
            | "caller"
    ) || [
        "password",
        "secret",
        "token",
        "authorization",
        "cookie",
        "api_key",
    ]
    .iter()
    .any(|sensitive| lower.contains(sensitive))
}

fn field_label(key: &str) -> String {
    match key {
        "message" | "msg" => "Problem".to_string(),
        "error" | "err" | "reason" => "Cause".to_string(),
        "otelcol.component.id" => "Component".to_string(),
        "otelcol.signal" => "Signal".to_string(),
        _ => {
            let words: String = key
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' {
                        c
                    } else {
                        ' '
                    }
                })
                .collect();
            let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut chars = words.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}
