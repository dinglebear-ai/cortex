//! AdGuard Home query log parser. Spec §7.5 + §8 (dual path).

use serde_json::{Map, Value, json};

use crate::enrich::{Parser, ParserError, ParserInput, ParserOutput};

pub struct AdguardParser;

impl Parser for AdguardParser {
    fn name(&self) -> &'static str {
        "adguard"
    }

    fn namespace(&self) -> &'static str {
        "adguard"
    }

    fn parse(&self, input: ParserInput<'_>) -> Result<ParserOutput, ParserError> {
        let value: Value = serde_json::from_str(input.message.trim())?;
        let obj = value
            .as_object()
            .ok_or(ParserError::Structural("not a json object"))?;

        // PascalCase (modern) with legacy camelCase fallback.
        let query = pick_str(obj, &["QH"]).or_else(|| nested_str(obj, "question", "host"));
        let qtype = pick_str(obj, &["QT"]).or_else(|| nested_str(obj, "question", "type"));
        // Client identity: AdGuard Home's file query log (≥0.107) records the
        // client address as `IP`; `Client`/`client` is the older API-poller
        // field. `CID` carries the persistent client id when configured.
        let client = pick_str(obj, &["Client", "client", "IP", "CID"]);
        let upstream = pick_str(obj, &["Upstream"]);
        let elapsed = pick_str(obj, &["Elapsed"]);
        let cached = obj.get("Cached").and_then(|v| v.as_bool());

        let result = obj
            .get("Result")
            .or_else(|| obj.get("result"))
            .and_then(|v| v.as_object())
            .ok_or(ParserError::Structural("missing Result"))?;
        let reason = pick_str(result, &["Reason", "reason"]);
        let rule = pick_str(result, &["Rule"]);
        let is_filtered = result
            .get("IsFiltered")
            .or_else(|| result.get("filtered"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Rewrite = neither blocked nor allowed; use None.
        let dns_blocked = match reason.as_deref() {
            Some(r) if r.starts_with("Rewrite") => None,
            _ if is_filtered => Some(true),
            _ => Some(false),
        };

        let mut metadata = Map::new();
        if let Some(q) = query {
            metadata.insert("query".into(), Value::String(q));
        }
        if let Some(t) = qtype {
            metadata.insert("qtype".into(), Value::String(t));
        }
        if let Some(c) = client {
            metadata.insert("client".into(), Value::String(c));
        }
        if let Some(u) = upstream {
            metadata.insert("upstream".into(), Value::String(u));
        }
        if let Some(r) = reason {
            metadata.insert("reason".into(), Value::String(r));
        }
        if let Some(r) = rule {
            metadata.insert("rule".into(), Value::String(r));
        }
        if let Some(e) = elapsed
            && let Some(stripped) = e.strip_suffix('s')
            && let Ok(secs) = stripped.parse::<f64>()
        {
            metadata.insert("elapsed_ms".into(), json!(secs * 1000.0));
        }
        if let Some(c) = cached {
            metadata.insert("cached".into(), json!(c));
        }

        Ok(ParserOutput {
            dns_blocked,
            event_action: Some("dns_query".into()),
            metadata,
            ..Default::default()
        })
    }
}

fn pick_str(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| obj.get(*k)?.as_str().map(str::to_string))
}

fn nested_str(obj: &Map<String, Value>, outer: &str, inner: &str) -> Option<String> {
    obj.get(outer)?
        .as_object()?
        .get(inner)?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
#[path = "adguard_tests.rs"]
mod adguard_tests;
