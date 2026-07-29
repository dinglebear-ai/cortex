//! SWAG / nginx access + error log parser. Spec §7.4.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct SwagParser;

const PATH_MAX: usize = 2048;
const UA_MAX: usize = 512;

/// nginx combined access log with optional upstream extras.
static ACCESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?x)
        ^(?P<client>\[?[^\s\]]+\]?) \s+ -               # client IP (plain or [ipv6])
        \s+ (?P<user>\S+)                                # remote user
        \s+ \[(?P<time>[^\]]+)\]                         # timestamp
        \s+ "(?P<method>\S+) \s (?P<path>[^"]*) \s HTTP/[^"]*" # request
        \s+ (?P<status>\d{3})                            # status
        \s+ (?P<bytes>\d+)                               # bytes
        \s+ "(?P<ref>[^"]*)"                             # referrer
        \s+ "(?P<ua>[^"]*)"                              # user-agent
        (?:\s+ "(?P<xff>[^"]*)" \s+ (?P<rt>[\d.]+))?   # optional: x-forwarded-for + request_time
        "#,
    )
    .expect("static regex")
});

/// nginx error log mentioning an upstream.
static ERROR_UPSTREAM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"upstream.*?upstream:\s+"(?P<upstream>[^"]+)""#).expect("static regex")
});

impl Parser for SwagParser {
    fn name(&self) -> &'static str {
        "swag"
    }

    fn namespace(&self) -> &'static str {
        "swag"
    }

    fn parse(&self, input: ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let msg = input.message;
        if let Some(caps) = ACCESS_RE.captures(msg) {
            return parse_access(caps);
        }
        if msg.contains(" [error] ") && msg.contains("upstream") {
            return parse_upstream_error(msg);
        }
        Err(ParserError::NoMatch("not an access or upstream-error line"))
    }
}

fn parse_access(caps: regex::Captures) -> Result<ParserOutput, ParserError> {
    let status: i32 = caps["status"]
        .parse()
        .map_err(|_| ParserError::MissingField("http_status"))?;
    let bytes: i64 = caps["bytes"].parse().unwrap_or(0);

    // Strip surrounding [ ] from IPv6 addresses.
    let client_raw = &caps["client"];
    let client = client_raw.trim_start_matches('[').trim_end_matches(']');

    let mut path = caps["path"].to_string();
    if path.len() > PATH_MAX {
        let mut n = PATH_MAX;
        while !path.is_char_boundary(n) {
            n -= 1;
        }
        path.truncate(n);
    }

    let mut ua = caps["ua"].to_string();
    if ua.len() > UA_MAX {
        let mut n = UA_MAX;
        while !ua.is_char_boundary(n) {
            n -= 1;
        }
        ua.truncate(n);
    }

    let mut metadata = Map::new();
    metadata.insert("method".into(), json!(&caps["method"]));
    metadata.insert("path".into(), json!(path));
    metadata.insert("client_ip".into(), json!(client));
    metadata.insert("bytes_sent".into(), json!(bytes));
    metadata.insert("referrer".into(), json!(&caps["ref"]));
    metadata.insert("user_agent".into(), json!(ua));

    if let Some(xff) = caps.name("xff") {
        metadata.insert("forwarded_for".into(), json!(xff.as_str()));
    }
    if let Some(rt) = caps.name("rt")
        && let Ok(secs) = rt.as_str().parse::<f64>()
    {
        metadata.insert("latency_ms".into(), json!((secs * 1000.0) as i32));
    }

    Ok(ParserOutput {
        http_status: Some(status),
        event_action: Some("http_request".into()),
        metadata,
        ..Default::default()
    })
}

fn parse_upstream_error(msg: &str) -> Result<ParserOutput, ParserError> {
    let caps = ERROR_UPSTREAM_RE
        .captures(msg)
        .ok_or(ParserError::NoMatch("upstream error format unrecognised"))?;
    let upstream = caps["upstream"].to_string();
    let error_class = if msg.contains("timed out") {
        "timeout"
    } else if msg.contains("Connection refused") || msg.contains("connrefused") {
        "connrefused"
    } else if msg.contains("Connection reset") {
        "reset"
    } else {
        "other"
    };
    let mut metadata = Map::new();
    metadata.insert("upstream".into(), Value::String(upstream));
    metadata.insert("error_class".into(), json!(error_class));
    Ok(ParserOutput {
        event_action: Some("upstream_error".into()),
        severity: Some("err"),
        metadata,
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "swag_tests.rs"]
mod swag_tests;
