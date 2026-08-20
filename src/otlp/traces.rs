//! OTLP trace-span normalization into the Agent Observatory DB input contract.
//!
//! This module owns the pure, deterministic conversion of one protobuf span;
//! the HTTP receiver applies it before idempotent persistence.

use std::fmt::Write as _;

use opentelemetry_proto::tonic::{
    common::v1::InstrumentationScope, resource::v1::Resource, trace::v1::Span,
};
use serde_json::json;
use thiserror::Error;

use crate::config::AgentObservatoryPrivacyConfig;
use crate::db::otlp_traces::OtelSpanInput;

use super::normalization::{MAX_RESOURCE_ATTRIBUTES, MAX_SIGNAL_ATTRIBUTES, normalize_attributes};
use super::privacy::{private_attributes, private_text};

#[path = "traces_payload.rs"]
mod payload;
use payload::{serialize_events, serialize_links};

const MAX_SPAN_NAME_CHARS: usize = 1024;
const MAX_STATUS_MESSAGE_CHARS: usize = 4096;
const MAX_HOSTNAME_CHARS: usize = 255;
const MAX_SERVICE_NAME_CHARS: usize = 512;
const MAX_SERVICE_VERSION_CHARS: usize = 512;
const MAX_SCOPE_NAME_CHARS: usize = 512;
const MAX_SCOPE_VERSION_CHARS: usize = 512;
const MAX_TRACE_STATE_CHARS: usize = 512;
const MAX_METADATA_JSON_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum TraceNormalizeError {
    #[error("{field} must be exactly {expected} non-zero bytes; got {actual}")]
    InvalidId {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("{field} contains {actual} attributes; maximum is {maximum}")]
    AttributeLimit {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    #[error("{field} exceeds maximum length {maximum}")]
    FieldTooLong { field: &'static str, maximum: usize },
    #[error("{field} does not fit SQLite INTEGER")]
    IntegerOverflow { field: &'static str },
    #[error("span end time precedes start time")]
    EndBeforeStart,
    #[error("received_at must be RFC3339")]
    InvalidReceivedAt,
    #[error("{field} JSON is {actual} bytes; maximum is {maximum}")]
    MetadataTooLarge {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
}

fn hex_id(
    bytes: &[u8],
    expected: usize,
    field: &'static str,
) -> Result<String, TraceNormalizeError> {
    if bytes.len() != expected || bytes.iter().all(|byte| *byte == 0) {
        return Err(TraceNormalizeError::InvalidId {
            field,
            expected,
            actual: bytes.len(),
        });
    }
    let mut encoded = String::with_capacity(expected * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing hex to String cannot fail");
    }
    Ok(encoded)
}

fn optional_parent_id(bytes: &[u8]) -> Result<Option<String>, TraceNormalizeError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    hex_id(bytes, 8, "parent_span_id").map(Some)
}

fn checked_i64(value: u64, field: &'static str) -> Result<i64, TraceNormalizeError> {
    i64::try_from(value).map_err(|_| TraceNormalizeError::IntegerOverflow { field })
}

fn optional_text(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn check_chars(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), TraceNormalizeError> {
    if value.chars().count() > maximum {
        return Err(TraceNormalizeError::FieldTooLong { field, maximum });
    }
    Ok(())
}

fn encode_json(
    value: serde_json::Value,
    field: &'static str,
) -> Result<String, TraceNormalizeError> {
    let encoded = value.to_string();
    if encoded.len() > MAX_METADATA_JSON_BYTES {
        return Err(TraceNormalizeError::MetadataTooLarge {
            field,
            actual: encoded.len(),
            maximum: MAX_METADATA_JSON_BYTES,
        });
    }
    Ok(encoded)
}

fn entity_refs(resource: Option<&Resource>) -> serde_json::Value {
    json!(resource.map_or_else(Vec::new, |resource| {
        resource
            .entity_refs
            .iter()
            .map(|entity| {
                json!({
                    "schema_url": private_text(&entity.schema_url),
                    "type": private_text(&entity.r#type),
                    "id_keys": entity.id_keys.iter().map(|key| private_text(key)).collect::<Vec<_>>(),
                    "description_keys": entity.description_keys.iter().map(|key| private_text(key)).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>()
    }))
}

fn resource_scope_json(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    privacy: &AgentObservatoryPrivacyConfig,
) -> Result<String, TraceNormalizeError> {
    let resource_attributes = resource.map_or_else(
        || json!({}),
        |resource| private_attributes(&resource.attributes, MAX_RESOURCE_ATTRIBUTES, privacy),
    );
    let scope_attributes = scope.map_or_else(
        || json!({}),
        |scope| private_attributes(&scope.attributes, MAX_RESOURCE_ATTRIBUTES, privacy),
    );
    let resource_schema_url = private_text(resource_schema_url);
    let scope_schema_url = private_text(scope_schema_url);
    encode_json(
        json!({
            "resource": {
                "schema_url": resource_schema_url,
                "attributes": resource_attributes,
                "dropped_attributes_count": resource.map_or(0, |resource| resource.dropped_attributes_count),
                "entity_refs": entity_refs(resource),
            },
            "scope": {
                "schema_url": scope_schema_url,
                "name": scope.map(|scope| private_text(&scope.name)),
                "version": scope.map(|scope| private_text(&scope.version)),
                "attributes": scope_attributes,
                "dropped_attributes_count": scope.map_or(0, |scope| scope.dropped_attributes_count),
            }
        }),
        "resource",
    )
}

/// Normalize one OTLP protobuf span with the default Agent Observatory privacy policy.
#[cfg(test)]
pub(crate) fn normalize_span(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    span: &Span,
    received_at: &str,
) -> Result<OtelSpanInput, TraceNormalizeError> {
    normalize_span_with_privacy(
        resource,
        resource_schema_url,
        scope,
        scope_schema_url,
        span,
        &AgentObservatoryPrivacyConfig::default(),
        received_at,
    )
}

/// Normalize one OTLP protobuf span into the migration-46 DB input shape while
/// applying the runtime Agent Observatory privacy policy.
pub(crate) fn normalize_span_with_privacy(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    span: &Span,
    privacy: &AgentObservatoryPrivacyConfig,
    received_at: &str,
) -> Result<OtelSpanInput, TraceNormalizeError> {
    let trace_id = hex_id(&span.trace_id, 16, "trace_id")?;
    let span_id = hex_id(&span.span_id, 8, "span_id")?;
    let parent_span_id = optional_parent_id(&span.parent_span_id)?;
    let start = checked_i64(span.start_time_unix_nano, "start_time_unix_nano")?;
    let end = checked_i64(span.end_time_unix_nano, "end_time_unix_nano")?;
    if end < start {
        return Err(TraceNormalizeError::EndBeforeStart);
    }
    let duration = end - start;

    let resource_attribute_count = resource.map_or(0, |resource| resource.attributes.len());
    if resource_attribute_count > MAX_RESOURCE_ATTRIBUTES {
        return Err(TraceNormalizeError::AttributeLimit {
            field: "resource",
            actual: resource_attribute_count,
            maximum: MAX_RESOURCE_ATTRIBUTES,
        });
    }
    let scope_attribute_count = scope.map_or(0, |scope| scope.attributes.len());
    if scope_attribute_count > MAX_RESOURCE_ATTRIBUTES {
        return Err(TraceNormalizeError::AttributeLimit {
            field: "scope",
            actual: scope_attribute_count,
            maximum: MAX_RESOURCE_ATTRIBUTES,
        });
    }
    if span.attributes.len() > MAX_SIGNAL_ATTRIBUTES {
        return Err(TraceNormalizeError::AttributeLimit {
            field: "span",
            actual: span.attributes.len(),
            maximum: MAX_SIGNAL_ATTRIBUTES,
        });
    }
    chrono::DateTime::parse_from_rfc3339(received_at)
        .map_err(|_| TraceNormalizeError::InvalidReceivedAt)?;
    check_chars(&span.name, MAX_SPAN_NAME_CHARS, "span_name")?;
    check_chars(&span.trace_state, MAX_TRACE_STATE_CHARS, "trace_state")?;

    let normalized = normalize_attributes(
        resource.map_or(&[], |resource| resource.attributes.as_slice()),
        &span.attributes,
    );
    check_chars(&normalized.host_name, MAX_HOSTNAME_CHARS, "hostname")?;
    if let Some(service_name) = normalized.service_name.as_deref() {
        check_chars(service_name, MAX_SERVICE_NAME_CHARS, "service_name")?;
    }
    if let Some(service_version) = normalized.service_version.as_deref() {
        check_chars(
            service_version,
            MAX_SERVICE_VERSION_CHARS,
            "service_version",
        )?;
    }
    if let Some(scope) = scope {
        check_chars(&scope.name, MAX_SCOPE_NAME_CHARS, "scope_name")?;
        check_chars(&scope.version, MAX_SCOPE_VERSION_CHARS, "scope_version")?;
    }

    let status_code = span
        .status
        .as_ref()
        .map_or(0, |status| i64::from(status.code));
    let status_message = span
        .status
        .as_ref()
        .and_then(|status| optional_text(&status.message));
    if let Some(status_message) = status_message.as_deref() {
        check_chars(status_message, MAX_STATUS_MESSAGE_CHARS, "status_message")?;
    }

    let resource_json = resource_scope_json(
        resource,
        resource_schema_url,
        scope,
        scope_schema_url,
        privacy,
    )?;
    let attributes_json = encode_json(
        private_attributes(&span.attributes, MAX_SIGNAL_ATTRIBUTES, privacy),
        "attributes",
    )?;
    let events_json = serialize_events(span, privacy)?;
    let links_json = serialize_links(span, privacy)?;
    let trace_state = optional_text(&span.trace_state).map(|value| private_text(&value));
    let span_name = private_text(&span.name);
    let status_message = status_message.map(|value| private_text(&value));
    let hostname = private_text(&normalized.host_name);
    let service_name = normalized.service_name.map(|value| private_text(&value));
    let service_version = normalized.service_version.map(|value| private_text(&value));
    let scope_name = scope
        .and_then(|scope| optional_text(&scope.name))
        .map(|value| private_text(&value));
    let scope_version = scope
        .and_then(|scope| optional_text(&scope.version))
        .map(|value| private_text(&value));
    let ai_tool = normalized.ai_tool.map(|value| private_text(&value));
    let ai_project = privacy
        .include_paths
        .then_some(normalized.ai_project)
        .flatten()
        .map(|value| private_text(&value));
    let ai_session_id = normalized.ai_session_id.map(|value| private_text(&value));

    Ok(OtelSpanInput {
        trace_id,
        span_id,
        parent_span_id,
        trace_state,
        flags: i64::from(span.flags),
        span_name,
        span_kind: i64::from(span.kind),
        start_time_unix_nano: start,
        end_time_unix_nano: end,
        duration_nano: duration,
        status_code,
        status_message,
        hostname,
        service_name,
        service_version,
        scope_name,
        scope_version,
        ai_tool,
        ai_project,
        ai_session_id,
        run_id: None,
        resource_json,
        attributes_json,
        events_json,
        links_json,
        received_at: received_at.to_string(),
        content_scrubbed: true,
    })
}

#[cfg(test)]
#[path = "traces_tests.rs"]
mod tests;
