//! Apprise HTTP client for sending push notifications.
//!
//! Security: NEVER log URLs, X-Apprise-Token, or Authorization headers at any
//! RUST_LOG level. Log rule_id, hostname, severity only.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Type of notification (maps to Apprise notify_type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyType {
    Info,
    Success,
    Warning,
    Failure,
}

impl NotifyType {
    fn as_str(self) -> &'static str {
        match self {
            NotifyType::Info => "info",
            NotifyType::Success => "success",
            NotifyType::Warning => "warning",
            NotifyType::Failure => "failure",
        }
    }
}

/// Response from Apprise notify endpoint.
#[derive(Debug, Clone)]
pub struct NotifyResponse {
    pub status_code: u16,
}

/// Errors that can occur when notifying via Apprise.
#[derive(Debug)]
pub enum AppriseError {
    /// Transient error — safe to retry (5xx, timeout, connection refused).
    Transient(String),
    /// Permanent error — do NOT retry (4xx except 429).
    Permanent { code: u16, body: String },
    /// The request timed out.
    Timeout,
}

impl std::fmt::Display for AppriseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppriseError::Transient(msg) => write!(f, "transient: {msg}"),
            AppriseError::Permanent { code, body } => write!(f, "permanent {code}: {body}"),
            AppriseError::Timeout => write!(f, "timeout"),
        }
    }
}

impl std::error::Error for AppriseError {}

/// HTTP client wrapper for the Apprise stateless notify endpoint.
#[derive(Clone)]
pub struct AppriseClient {
    client: reqwest::Client,
    /// Base URL of apprise-api, e.g. "http://apprise:8000".
    base_url: String,
    /// Request timeout.
    timeout: Duration,
}

impl AppriseClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(false)
            .build()
            .expect("reqwest client should build");
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(5),
        }
    }

    #[cfg(test)]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// POST a notification to `{base_url}/notify/`.
    ///
    /// Apprise stateless mode: URLs are sent in the request body, not stored
    /// server-side.
    pub async fn notify(
        &self,
        urls: &[String],
        title: &str,
        body: &str,
        notify_type: NotifyType,
    ) -> Result<NotifyResponse, AppriseError> {
        let safe_title = escape_for_notification(title);
        let safe_body = escape_for_notification(body);

        let endpoint = format!("{}/notify/", self.base_url);
        let payload = serde_json::json!({
            "urls": urls,
            "title": safe_title,
            "body": safe_body,
            "type": notify_type.as_str(),
            "format": "markdown",
        });

        let request = self
            .client
            .post(&endpoint)
            // .json() already sets Content-Type: application/json
            .json(&payload)
            .timeout(self.timeout);

        let resp = request.send().await.map_err(|e| {
            if e.is_timeout() {
                AppriseError::Timeout
            } else {
                // Use without_url() to avoid leaking Apprise base URLs
                // (which may contain credentials) in tracing/error output.
                AppriseError::Transient(format!("send error: {}", e.without_url()))
            }
        })?;

        let status = resp.status().as_u16();

        match status {
            // 200/201/202 = success; 207 = partial success — mark sent, do NOT retry
            // 204 = No Content: nothing was sent (no targets / empty body) — permanent failure
            200 | 201 | 202 | 207 => Ok(NotifyResponse {
                status_code: status,
            }),
            429 | 500..=599 => {
                // Transient — safe to retry
                Err(AppriseError::Transient(format!("HTTP {status}")))
            }
            300..=399 => Err(AppriseError::Transient(format!("redirect HTTP {status}"))),
            other => {
                // 4xx (excluding 429) = permanent
                let body_text = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "<body read error>".to_string());
                // Truncate body to avoid logging sensitive content
                let truncated = body_text.chars().take(200).collect::<String>();
                Err(AppriseError::Permanent {
                    code: other,
                    body: truncated,
                })
            }
        }
    }
}

/// Escape log-derived text for safe notification delivery.
///
/// The payload is sent with `"format": "markdown"`, so raw `<`/`>` would be
/// interpreted as HTML tags by markdown-rendering targets (Gotify, etc.) and
/// silently dropped. The error-signature normalizer emits placeholder tokens
/// like `<n>`, `<hex>`, `<ip>`, `<path>`, so stripping the brackets mangled
/// every signature into unreadable runs (`<n>:<n>:<n>` → `n:n:n`). Instead we
/// HTML-escape the markup characters: this both renders the placeholders
/// literally and still neutralises tag/markup injection.
///
/// - Replaces `@` with `＠` (U+FF20) to prevent mention injection.
/// - Escapes `&`, `<`, `>` to their HTML entities so they render verbatim.
pub fn escape_for_notification(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '@' => out.push('＠'),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
#[path = "apprise_tests.rs"]
mod tests;
