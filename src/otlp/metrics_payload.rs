//! Canonical value, exemplar, and point-key encoding for OTLP metrics.

use std::fmt::Write as _;

use opentelemetry_proto::tonic::metrics::v1::{Exemplar, exemplar, number_data_point};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::AgentObservatoryPrivacyConfig;

use super::metrics::{MetricNormalizeError, checked_i64};
use super::normalization::MAX_SIGNAL_ATTRIBUTES;
use super::privacy::private_attributes;

pub(super) fn serialize_exemplars(
    exemplars: &[Exemplar],
    privacy: &AgentObservatoryPrivacyConfig,
) -> Result<Value, MetricNormalizeError> {
    exemplars
        .iter()
        .map(|value| {
            Ok(json!({
                "filtered_attributes": private_attributes(&value.filtered_attributes, MAX_SIGNAL_ATTRIBUTES, privacy),
                "time_unix_nano": checked_i64(value.time_unix_nano, "exemplar.time_unix_nano")?,
                "trace_id": optional_hex_id(&value.trace_id, 16, "exemplar.trace_id")?,
                "span_id": optional_hex_id(&value.span_id, 8, "exemplar.span_id")?,
                "value": value.value.as_ref().map(exemplar_value).unwrap_or(Value::Null),
            }))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

pub(super) fn exemplar_ids(exemplars: &[Exemplar]) -> Result<Vec<String>, MetricNormalizeError> {
    let mut ids = exemplars
        .iter()
        .map(|value| {
            let trace =
                optional_hex_id(&value.trace_id, 16, "exemplar.trace_id")?.unwrap_or_default();
            let span = optional_hex_id(&value.span_id, 8, "exemplar.span_id")?.unwrap_or_default();
            Ok(format!("{trace}:{span}"))
        })
        .collect::<Result<Vec<_>, MetricNormalizeError>>()?;
    ids.sort();
    Ok(ids)
}

pub(super) fn number_value(value: &number_data_point::Value, flags: u32) -> Value {
    match value {
        number_data_point::Value::AsInt(value) => {
            json!({"type": "int", "value": value, "flags": flags})
        }
        number_data_point::Value::AsDouble(value) => {
            json!({"type": "double", "value": safe_double(*value), "flags": flags})
        }
    }
}

fn exemplar_value(value: &exemplar::Value) -> Value {
    match value {
        exemplar::Value::AsInt(value) => json!({"type": "int", "value": value}),
        exemplar::Value::AsDouble(value) => json!({"type": "double", "value": safe_double(*value)}),
    }
}

pub(super) fn safe_double(value: f64) -> Value {
    if value.is_nan() {
        return Value::String("nan".to_string());
    }
    if value == f64::INFINITY {
        return Value::String("+infinity".to_string());
    }
    if value == f64::NEG_INFINITY {
        return Value::String("-infinity".to_string());
    }
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn optional_hex_id(
    bytes: &[u8],
    expected: usize,
    field: &'static str,
) -> Result<Option<String>, MetricNormalizeError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() != expected || bytes.iter().all(|byte| *byte == 0) {
        return Err(MetricNormalizeError::InvalidOptionalId {
            field,
            expected,
            actual: bytes.len(),
        });
    }
    let mut encoded = String::with_capacity(expected * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing hex to String cannot fail");
    }
    Ok(Some(encoded))
}

pub(super) struct PointKeyParts<'a> {
    pub resource: &'a Value,
    pub scope: &'a Value,
    pub metric_name: &'a str,
    pub instrument_kind: &'a str,
    pub unit: &'a str,
    pub aggregation_temporality: Option<i32>,
    pub monotonic: Option<bool>,
    pub start_time_unix_nano: Option<i64>,
    pub time_unix_nano: i64,
    pub attributes: &'a Value,
    pub value: &'a Value,
    pub exemplar_ids: &'a [String],
}

pub(super) fn point_key(parts: PointKeyParts<'_>) -> String {
    let resource_encoded = parts.resource.to_string();
    let resource_fingerprint = format!("{:x}", Sha256::digest(resource_encoded.as_bytes()));
    let mut hasher = Sha256::new();
    for component in [
        resource_fingerprint,
        parts.scope.to_string(),
        parts.metric_name.to_string(),
        parts.instrument_kind.to_string(),
        parts.unit.to_string(),
        parts
            .aggregation_temporality
            .map_or_else(String::new, |value| value.to_string()),
        parts
            .monotonic
            .map_or_else(String::new, |value| value.to_string()),
        parts
            .start_time_unix_nano
            .map_or_else(String::new, |value| value.to_string()),
        parts.time_unix_nano.to_string(),
        parts.attributes.to_string(),
        parts.value.to_string(),
        parts.exemplar_ids.join(","),
    ] {
        hash_component(&mut hasher, &component);
    }
    format!("{:x}", hasher.finalize())
}

fn hash_component(hasher: &mut Sha256, value: &str) {
    let length = u64::try_from(value.len()).expect("metric point key component length fits u64");
    hasher.update(length.to_be_bytes());
    hasher.update(value.as_bytes());
}
