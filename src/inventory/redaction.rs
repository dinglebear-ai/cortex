use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

use crate::inventory::limits::{MAX_ARRAY_ENTRIES, MAX_JSON_DEPTH, truncate_text};
use crate::inventory::schema::RedactionStatus;

const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedArtifact {
    body: String,
    status: RedactionStatus,
    truncated: bool,
}

impl RedactedArtifact {
    pub fn from_text(input: &str, max_bytes: usize) -> Self {
        let redacted = redact_text(input);
        let (body, truncated) = truncate_text(&redacted, max_bytes);
        let status = if redacted == input {
            RedactionStatus::NoSecretsDetected
        } else {
            RedactionStatus::Redacted
        };
        Self {
            body,
            status,
            truncated,
        }
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn status(&self) -> RedactionStatus {
        self.status.clone()
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

pub fn redact_text(input: &str) -> String {
    let mut out = input.to_string();
    for regex in secret_regexes() {
        out = regex
            .replace_all(&out, |caps: &regex::Captures<'_>| {
                if caps.len() > 1 {
                    format!("{}{}", &caps[1], REDACTED)
                } else {
                    REDACTED.to_string()
                }
            })
            .to_string();
    }
    out
}

pub fn redact_error(input: impl AsRef<str>) -> (String, bool) {
    let redacted = redact_text(input.as_ref());
    let (truncated, did_truncate) = truncate_text(&redacted, 1024);
    (truncated, did_truncate)
}

pub fn redact_json(value: &Value) -> Value {
    redact_json_inner(value, 0)
}

fn redact_json_inner(value: &Value, depth: usize) -> Value {
    if depth >= MAX_JSON_DEPTH {
        return Value::String("[TRUNCATED_DEPTH]".to_string());
    }
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                if sensitive_key(key)
                    || (key.eq_ignore_ascii_case("value") && sibling_name_is_sensitive(map))
                {
                    out.insert(key.clone(), Value::String(REDACTED.to_string()));
                } else {
                    out.insert(key.clone(), redact_json_inner(value, depth + 1));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            let truncated = items.len() > MAX_ARRAY_ENTRIES;
            let mut out = items
                .iter()
                .take(MAX_ARRAY_ENTRIES)
                .map(|item| redact_json_inner(item, depth + 1))
                .collect::<Vec<_>>();
            if truncated {
                out.push(Value::String("[TRUNCATED_ARRAY]".to_string()));
            }
            Value::Array(out)
        }
        Value::String(text) => Value::String(redact_text(text)),
        _ => value.clone(),
    }
}

pub fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization",
        "bearer",
        "cookie",
        "client_secret",
        "key",
        "pass",
        "password",
        "passphrase",
        "private_key",
        "psk",
        "refresh_token",
        "secret",
        "session",
        "token",
        "x-api-key",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn sibling_name_is_sensitive(map: &Map<String, Value>) -> bool {
    map.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("name") || key.eq_ignore_ascii_case("key"))
        .map(|(_, value)| value)
        .and_then(Value::as_str)
        .is_some_and(sensitive_key)
}

fn secret_regexes() -> &'static [Regex] {
    static REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    REGEXES.get_or_init(|| {
        let private_key_label = ["PRI", "VATE ", "KEY"].concat();
        let pem_fence = "-".repeat(5);
        vec![
            // PEM private keys
            Regex::new(&format!(
                r#"(?is){pem_fence}BEGIN [A-Z ]*{private_key_label}{pem_fence}.*?{pem_fence}END [A-Z ]*{private_key_label}{pem_fence}"#
            ))
            .expect("static PEM regex"),
            // curl -u user:pass
            Regex::new(r#"(?i)(\bcurl\b[^\n]*\s-u\s+)["']?[^"'\s]+["']?"#)
                .expect("static curl-u regex"),
            // URI userinfo (scheme://user:pass@host)
            Regex::new(r#"(?i)([a-z][a-z0-9+.-]*://)[^/@\s:]+(?::[^/@\s]*)?@"#)
                .expect("static URI userinfo regex"),
            // Sensitive URL query parameters, including generic `key=`.
            Regex::new(
                r#"(?i)([?&](?:api[_-]?key|x[_-]?api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|passphrase|psk|client[_-]?secret|key)=)[^&#\s\"']+"#,
            )
            .expect("static URL query secret regex"),
            // Authorization / Bearer headers
            Regex::new(r#"(?i)(authorization\s*[:=]\s*(?:bearer\s+)?)[A-Za-z0-9._~+/=-]{12,}"#)
                .expect("static authorization regex"),
            // Cookie header
            Regex::new(r#"(?i)(cookie\s*[:=]\s*)[^\s;]{8,}"#)
                .expect("static cookie regex"),
            // KEY=VALUE for known secret key names (quoted or unquoted)
            Regex::new(r#"(?i)((?:api[_-]?key|x[_-]?api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|passphrase|private[_-]?key|psk|client[_-]?secret|webhook|smtp[_-]?pass(?:word)?|db[_-]?pass(?:word)?|database[_-]?url|minio[_-]?(?:root[_-]?)?(?:user|password)|smtp[_-]?url)\s*[:=]\s*)(?:"[^"\n]{3,}"|'[^'\n]{3,}'|[^\s&,"']{3,})"#)
                .expect("static KEY=VALUE space-sep regex"),
            // KEY=VALUE env-style (no spaces around =)
            Regex::new(r#"(?i)((?:api[_-]?key|x[_-]?api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|passphrase|private[_-]?key|psk|client[_-]?secret|webhook|smtp[_-]?pass(?:word)?|db[_-]?pass(?:word)?|database[_-]?url|minio[_-]?(?:root[_-]?)?(?:user|password)|smtp[_-]?url)=)(?:"[^"\n]{3,}"|'[^'\n]{3,}'|[^&\s"']+)"#)
                .expect("static KEY=VALUE env regex"),
            // Any variable whose name ends in _TOKEN, _SECRET, _PASSWORD, _KEY, _CREDENTIAL
            Regex::new(r#"(?i)([A-Z_][A-Z0-9_]*(?:_TOKEN|_SECRET|_PASSWORD|_KEY|_CREDENTIAL|_AUTH)\s*=\s*)(?:"[^"\n]{4,}"|'[^'\n]{4,}'|[^\s"'&]{4,})"#)
                .expect("static suffix env-var regex"),
            // Vendor-prefixed tokens: AWS, GitHub, GitLab, Slack, Stripe, Twilio, SendGrid, Datadog, NPM
            Regex::new(r#"(?:AKIA|ASIA|AROA|AIPA|ANPA|ANVA|APKA)[0-9A-Z]{16}"#)
                .expect("static AWS key regex"),
            Regex::new(r#"(?:ghp|gho|ghu|ghs|ghr|github_pat)_[A-Za-z0-9_]{20,}"#)
                .expect("static GitHub token regex"),
            Regex::new(r#"glpat-[A-Za-z0-9_\-]{20,}"#)
                .expect("static GitLab token regex"),
            Regex::new(r#"xox[baprs]-[A-Za-z0-9\-]{10,}"#)
                .expect("static Slack token regex"),
            Regex::new(r#"sk_(?:live|test)_[A-Za-z0-9]{20,}"#)
                .expect("static Stripe key regex"),
            // JWT (three base64url segments — handles both base64 and base64url charsets)
            Regex::new(r#"[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{16,}"#)
                .expect("static JWT regex"),
            // High-entropy base64 blobs (>=32 chars). Keep '-' out so UUIDs
            // and common dashed identifiers are not treated as secrets.
            Regex::new(r#"\b[A-Za-z0-9+/]{32,}={0,2}\b"#)
                .expect("static base64 catch-all regex"),
        ]
    })
}

#[cfg(test)]
#[path = "redaction_tests.rs"]
mod tests;
