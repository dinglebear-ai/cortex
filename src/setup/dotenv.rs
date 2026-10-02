//! Literal managed configuration shared by setup, CLI and Compose consumers.
//!
//! No shell execution or environment interpolation occurs while loading values.
//! Rendering quotes special values and escapes dollars so Compose sees the same
//! credential rather than expanding it against the installing user's environment.
use std::collections::BTreeMap;

pub fn encode(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./:@,+-=~".contains(c))
    {
        return value.to_owned();
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn decode(value: &str) -> Option<String> {
    let value = value.trim();
    let Some(quote @ ('\'' | '"')) = value.chars().next() else {
        let content = value.split_once(" #").map_or(value, |(content, _)| content);
        return Some(content.trim_end().to_owned());
    };
    let mut chars = value[1..].chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == quote {
            let tail: String = chars.collect();
            let tail = tail.trim();
            return (tail.is_empty() || tail.starts_with('#')).then_some(out);
        }
        if c == '\\' {
            let next = chars.next()?;
            if next == quote {
                out.push(next);
            } else if quote == '"' {
                match next {
                    '\\' => out.push('\\'),
                    '$' => out.push('$'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    _ => {
                        out.push('\\');
                        out.push(next);
                    }
                }
            } else {
                // Single quotes preserve backslashes except an escaped quote.
                out.push('\\');
                out.push(next);
            }
        } else if c == '$' && quote == '"' && chars.peek() == Some(&'$') {
            chars.next();
            out.push('$');
        } else {
            out.push(c);
        }
    }
    None
}

/// Preserve entry ordering so loaders can retain their duplicate-key precedence.
pub fn entries(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").map_or(line, str::trim_start);
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if key.is_empty() || key.contains('\0') || value.contains('\0') {
                return None;
            }
            Some((key.to_owned(), decode(value)?))
        })
        .collect()
}

pub fn parse(raw: &str) -> BTreeMap<String, String> {
    entries(raw).into_iter().collect()
}

/// Reuse unchanged assignments verbatim, retaining an existing operator's Compose
/// interpolation expressions. New or changed assignments are encoded as literals.
pub fn render_preserving(raw: &str, values: &BTreeMap<String, String>) -> String {
    let original = parse(raw);
    let mut assignments = BTreeMap::new();
    for line in raw.lines() {
        if let Some((key, _)) = entries(line).into_iter().next() {
            assignments.insert(key, line.to_owned());
        }
    }
    let mut out = String::from(
        "# cortex runtime environment.\n# Managed by `cortex setup`; secrets are preserved on repair.\n",
    );
    for (key, value) in values {
        if original.get(key) == Some(value)
            && let Some(line) = assignments.get(key)
        {
            out.push_str(line);
        } else {
            out.push_str(key);
            out.push('=');
            out.push_str(&encode(value));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
#[path = "dotenv_tests.rs"]
mod tests;
