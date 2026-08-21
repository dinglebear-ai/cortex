//! Pure OTLP gauge/sum normalization for Agent Observatory metric points.

use super::metrics_payload::{PointKeyParts, number_value, point_key, serialize_exemplars};
use super::normalization::{MAX_RESOURCE_ATTRIBUTES, MAX_SIGNAL_ATTRIBUTES, normalize_attributes};
use super::privacy::{private_attributes, private_text};
use crate::config::AgentObservatoryPrivacyConfig;
use opentelemetry_proto::tonic::{
    common::v1::{EntityRef, InstrumentationScope, KeyValue},
    metrics::v1::Exemplar,
    metrics::v1::{Metric, NumberDataPoint, metric},
    resource::v1::Resource,
};
use serde_json::{Value, json};
use thiserror::Error;

const MAX_METADATA_JSON_BYTES: usize = 256 * 1024;
const MAX_METRIC_NAME_CHARS: usize = 512;
const MAX_DESCRIPTION_CHARS: usize = 4096;
const MAX_UNIT_CHARS: usize = 128;
const MAX_SCOPE_NAME_CHARS: usize = 512;
const MAX_SCOPE_VERSION_CHARS: usize = 512;
const MAX_HOSTNAME_CHARS: usize = 255;
const MAX_SERVICE_NAME_CHARS: usize = 512;
const MAX_SERVICE_VERSION_CHARS: usize = 512;
pub(super) const MAX_EXEMPLARS: usize = 128;

#[path = "metrics_distribution.rs"]
mod distribution;
#[path = "metrics_histogram.rs"]
mod histogram;

#[cfg(test)]
pub(crate) use distribution::normalize_distribution_metric;
#[cfg(test)]
pub(crate) use histogram::normalize_histogram_metric;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetricPointInput {
    pub point_key: String,
    pub metric_name: String,
    pub description: String,
    pub unit: String,
    pub instrument_kind: String,
    pub aggregation_temporality: Option<i32>,
    pub monotonic: Option<bool>,
    pub start_time_unix_nano: Option<i64>,
    pub time_unix_nano: i64,
    pub hostname: String,
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub run_id: Option<i64>,
    pub resource_json: String,
    pub attributes_json: String,
    pub value_json: String,
    pub exemplars_json: String,
    pub received_at: String,
    pub content_scrubbed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum MetricNormalizeError {
    #[error("metric kind is unsupported or missing")]
    UnsupportedInstrument,
    #[error("metric name must not be empty")]
    EmptyMetricName,
    #[error("number data point has no value")]
    MissingValue,
    #[error("sum aggregation temporality must not be unspecified")]
    UnspecifiedTemporality,
    #[error("{field} contains {actual} attributes; maximum is {maximum}")]
    AttributeLimit {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    #[error("point contains {actual} exemplars; maximum is {maximum}")]
    ExemplarLimit { actual: usize, maximum: usize },
    #[error("histogram contains {actual} buckets; maximum is {maximum}")]
    HistogramBucketLimit { actual: usize, maximum: usize },
    #[error("histogram has {buckets} bucket counts and {bounds} explicit bounds")]
    InvalidHistogramShape { buckets: usize, bounds: usize },
    #[error("histogram bucket counts do not sum to the point count")]
    HistogramCountMismatch,
    #[error("histogram explicit bounds must be strictly increasing")]
    HistogramBoundsNotIncreasing,
    #[error("distribution contains {actual} values; maximum is {maximum}")]
    DistributionValueLimit { actual: usize, maximum: usize },
    #[error("exponential histogram bucket counts do not sum to the point count")]
    ExponentialHistogramCountMismatch,
    #[error("summary quantiles must be finite, ordered, and within 0..=1")]
    InvalidSummaryQuantiles,
    #[error("summary quantile values must be finite")]
    InvalidSummaryValue,
    #[error("{field} must be empty or exactly {expected} non-zero bytes; got {actual}")]
    InvalidOptionalId {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("{field} exceeds maximum length {maximum}")]
    FieldTooLong { field: &'static str, maximum: usize },
    #[error("{field} does not fit SQLite INTEGER")]
    IntegerOverflow { field: &'static str },
    #[error("point time must be non-zero")]
    MissingPointTime,
    #[error("point time precedes start time")]
    TimeBeforeStart,
    #[error("received_at must be RFC3339")]
    InvalidReceivedAt,
    #[error("{field} JSON is {actual} bytes; maximum is {maximum}")]
    MetadataTooLarge {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
}

pub(crate) fn normalize_metric_with_privacy(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    metric: &Metric,
    privacy: &AgentObservatoryPrivacyConfig,
    received_at: &str,
) -> Result<Vec<MetricPointInput>, MetricNormalizeError> {
    match metric.data.as_ref() {
        Some(metric::Data::Gauge(_) | metric::Data::Sum(_)) => {
            normalize_number_metric_with_privacy(
                resource,
                resource_schema_url,
                scope,
                scope_schema_url,
                metric,
                privacy,
                received_at,
            )
        }
        Some(metric::Data::Histogram(_)) => histogram::normalize_histogram_metric_with_privacy(
            resource,
            resource_schema_url,
            scope,
            scope_schema_url,
            metric,
            privacy,
            received_at,
        ),
        Some(metric::Data::ExponentialHistogram(_) | metric::Data::Summary(_)) => {
            distribution::normalize_distribution_metric_with_privacy(
                resource,
                resource_schema_url,
                scope,
                scope_schema_url,
                metric,
                privacy,
                received_at,
            )
        }
        None => Err(MetricNormalizeError::UnsupportedInstrument),
    }
}

#[derive(Clone, Copy)]
pub(super) struct NumberMetricContext<'a> {
    resource: Option<&'a Resource>,
    resource_schema_url: &'a str,
    scope: Option<&'a InstrumentationScope>,
    scope_schema_url: &'a str,
    metric: &'a Metric,
    privacy: &'a AgentObservatoryPrivacyConfig,
    received_at: &'a str,
    instrument_kind: &'static str,
    aggregation_temporality: Option<i32>,
    monotonic: Option<bool>,
    ignore_start_time: bool,
}

pub(super) struct PointParts<'a> {
    pub attributes: &'a [KeyValue],
    pub exemplars: &'a [Exemplar],
    pub start_time_unix_nano: u64,
    pub time_unix_nano: u64,
    pub value: Value,
}

#[cfg(test)]
pub(crate) fn normalize_number_metric(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    metric: &Metric,
    received_at: &str,
) -> Result<Vec<MetricPointInput>, MetricNormalizeError> {
    normalize_number_metric_with_privacy(
        resource,
        resource_schema_url,
        scope,
        scope_schema_url,
        metric,
        &AgentObservatoryPrivacyConfig::default(),
        received_at,
    )
}

pub(crate) fn normalize_number_metric_with_privacy(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    metric: &Metric,
    privacy: &AgentObservatoryPrivacyConfig,
    received_at: &str,
) -> Result<Vec<MetricPointInput>, MetricNormalizeError> {
    validate_metric_envelope(resource, scope, metric, received_at)?;
    let (instrument_kind, aggregation_temporality, monotonic, ignore_start_time) =
        match metric.data.as_ref() {
            Some(metric::Data::Gauge(_)) => ("gauge", None, None, true),
            Some(metric::Data::Sum(sum)) => {
                if sum.aggregation_temporality == 0 {
                    return Err(MetricNormalizeError::UnspecifiedTemporality);
                }
                (
                    "sum",
                    Some(sum.aggregation_temporality),
                    Some(sum.is_monotonic),
                    false,
                )
            }
            _ => return Err(MetricNormalizeError::UnsupportedInstrument),
        };
    let context = NumberMetricContext {
        resource,
        resource_schema_url,
        scope,
        scope_schema_url,
        metric,
        privacy,
        received_at,
        instrument_kind,
        aggregation_temporality,
        monotonic,
        ignore_start_time,
    };
    let points = match metric.data.as_ref() {
        Some(metric::Data::Gauge(gauge)) => &gauge.data_points,
        Some(metric::Data::Sum(sum)) => &sum.data_points,
        _ => unreachable!("instrument kind checked above"),
    };
    points
        .iter()
        .map(|point| normalize_number_point(context, point))
        .collect()
}

pub(super) fn validate_metric_envelope(
    resource: Option<&Resource>,
    scope: Option<&InstrumentationScope>,
    metric: &Metric,
    received_at: &str,
) -> Result<(), MetricNormalizeError> {
    if metric.name.is_empty() {
        return Err(MetricNormalizeError::EmptyMetricName);
    }
    check_chars(&metric.name, MAX_METRIC_NAME_CHARS, "metric_name")?;
    check_chars(&metric.description, MAX_DESCRIPTION_CHARS, "description")?;
    check_chars(&metric.unit, MAX_UNIT_CHARS, "unit")?;
    let resource_count = resource.map_or(0, |value| value.attributes.len());
    if resource_count > MAX_RESOURCE_ATTRIBUTES {
        return Err(MetricNormalizeError::AttributeLimit {
            field: "resource",
            actual: resource_count,
            maximum: MAX_RESOURCE_ATTRIBUTES,
        });
    }
    let scope_count = scope.map_or(0, |value| value.attributes.len());
    if scope_count > MAX_RESOURCE_ATTRIBUTES {
        return Err(MetricNormalizeError::AttributeLimit {
            field: "scope",
            actual: scope_count,
            maximum: MAX_RESOURCE_ATTRIBUTES,
        });
    }
    if let Some(scope) = scope {
        check_chars(&scope.name, MAX_SCOPE_NAME_CHARS, "scope_name")?;
        check_chars(&scope.version, MAX_SCOPE_VERSION_CHARS, "scope_version")?;
    }
    chrono::DateTime::parse_from_rfc3339(received_at)
        .map_err(|_| MetricNormalizeError::InvalidReceivedAt)?;
    Ok(())
}

fn normalize_number_point(
    context: NumberMetricContext<'_>,
    point: &NumberDataPoint,
) -> Result<MetricPointInput, MetricNormalizeError> {
    let value = number_value(
        point
            .value
            .as_ref()
            .ok_or(MetricNormalizeError::MissingValue)?,
        point.flags,
    );
    build_metric_point(
        context,
        PointParts {
            attributes: &point.attributes,
            exemplars: &point.exemplars,
            start_time_unix_nano: point.start_time_unix_nano,
            time_unix_nano: point.time_unix_nano,
            value,
        },
    )
}

pub(super) fn build_metric_point(
    context: NumberMetricContext<'_>,
    point: PointParts<'_>,
) -> Result<MetricPointInput, MetricNormalizeError> {
    if point.attributes.len() > MAX_SIGNAL_ATTRIBUTES {
        return Err(MetricNormalizeError::AttributeLimit {
            field: "point",
            actual: point.attributes.len(),
            maximum: MAX_SIGNAL_ATTRIBUTES,
        });
    }
    if point.exemplars.len() > MAX_EXEMPLARS {
        return Err(MetricNormalizeError::ExemplarLimit {
            actual: point.exemplars.len(),
            maximum: MAX_EXEMPLARS,
        });
    }
    if point.time_unix_nano == 0 {
        return Err(MetricNormalizeError::MissingPointTime);
    }
    let time_unix_nano = checked_i64(point.time_unix_nano, "time_unix_nano")?;
    let start_time_unix_nano = if context.ignore_start_time || point.start_time_unix_nano == 0 {
        None
    } else {
        Some(checked_i64(
            point.start_time_unix_nano,
            "start_time_unix_nano",
        )?)
    };
    if start_time_unix_nano.is_some_and(|start| time_unix_nano < start) {
        return Err(MetricNormalizeError::TimeBeforeStart);
    }
    let normalized = normalize_attributes(
        context
            .resource
            .map_or(&[], |value| value.attributes.as_slice()),
        point.attributes,
    );
    check_chars(&normalized.host_name, MAX_HOSTNAME_CHARS, "hostname")?;
    if let Some(value) = normalized.service_name.as_deref() {
        check_chars(value, MAX_SERVICE_NAME_CHARS, "service_name")?;
    }
    if let Some(value) = normalized.service_version.as_deref() {
        check_chars(value, MAX_SERVICE_VERSION_CHARS, "service_version")?;
    }

    let resource_value = resource_key_value(
        context.resource,
        context.resource_schema_url,
        context.privacy,
    );
    let scope_value = scope_key_value(context.scope, context.scope_schema_url, context.privacy);
    let resource_json_value = json!({
        "resource": resource_value.clone(),
        "scope": scope_value.clone(),
    });
    let attributes_value =
        private_attributes(point.attributes, MAX_SIGNAL_ATTRIBUTES, context.privacy);
    let value_value = point.value;
    let exemplars_value = serialize_exemplars(point.exemplars, context.privacy)?;
    let resource_json = encode_json(resource_json_value, "resource")?;
    let attributes_json = encode_json(attributes_value.clone(), "attributes")?;
    let value_json = encode_json(value_value.clone(), "value")?;
    let exemplars_json = encode_json(exemplars_value, "exemplars")?;
    let metric_name = private_text(&context.metric.name);
    let unit = private_text(&context.metric.unit);
    let point_key = point_key(PointKeyParts {
        resource: &resource_value,
        scope: &scope_value,
        metric_name: &metric_name,
        instrument_kind: context.instrument_kind,
        unit: &unit,
        aggregation_temporality: context.aggregation_temporality,
        monotonic: context.monotonic,
        start_time_unix_nano,
        time_unix_nano,
        attributes: &attributes_value,
    });

    Ok(MetricPointInput {
        point_key,
        metric_name,
        description: private_text(&context.metric.description),
        unit,
        instrument_kind: context.instrument_kind.to_string(),
        aggregation_temporality: context.aggregation_temporality,
        monotonic: context.monotonic,
        start_time_unix_nano,
        time_unix_nano,
        hostname: private_text(&normalized.host_name),
        service_name: normalized.service_name.map(|value| private_text(&value)),
        service_version: normalized.service_version.map(|value| private_text(&value)),
        scope_name: context
            .scope
            .and_then(|value| nonempty_private(&value.name)),
        scope_version: context
            .scope
            .and_then(|value| nonempty_private(&value.version)),
        ai_tool: normalized.ai_tool.map(|value| private_text(&value)),
        ai_project: context
            .privacy
            .include_paths
            .then_some(normalized.ai_project)
            .flatten()
            .map(|value| private_text(&value)),
        ai_session_id: normalized.ai_session_id.map(|value| private_text(&value)),
        run_id: None,
        resource_json,
        attributes_json,
        value_json,
        exemplars_json,
        received_at: context.received_at.to_string(),
        content_scrubbed: true,
    })
}

fn resource_key_value(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    privacy: &AgentObservatoryPrivacyConfig,
) -> Value {
    json!({
        "schema_url": private_text(resource_schema_url),
        "attributes": resource.map_or_else(|| json!({}), |value| private_attributes(&value.attributes, MAX_RESOURCE_ATTRIBUTES, privacy)),
        "dropped_attributes_count": resource.map_or(0, |value| value.dropped_attributes_count),
        "entity_refs": resource.map_or_else(Vec::new, |value| canonical_entity_refs(&value.entity_refs)),
    })
}

fn canonical_entity_refs(entity_refs: &[EntityRef]) -> Vec<Value> {
    let mut values = entity_refs
        .iter()
        .map(|entity| {
            let mut id_keys = entity.id_keys.clone();
            id_keys.sort();
            let mut description_keys = entity.description_keys.clone();
            description_keys.sort();
            json!({
                "schema_url": private_text(&entity.schema_url),
                "type": private_text(&entity.r#type),
                "id_keys": id_keys,
                "description_keys": description_keys,
            })
        })
        .collect::<Vec<_>>();
    values.sort_by_cached_key(Value::to_string);
    values
}

fn scope_key_value(
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    privacy: &AgentObservatoryPrivacyConfig,
) -> Value {
    json!({
        "schema_url": private_text(scope_schema_url),
        "name": scope.map(|value| private_text(&value.name)),
        "version": scope.map(|value| private_text(&value.version)),
        "attributes": scope.map_or_else(|| json!({}), |value| private_attributes(&value.attributes, MAX_RESOURCE_ATTRIBUTES, privacy)),
        "dropped_attributes_count": scope.map_or(0, |value| value.dropped_attributes_count),
    })
}

pub(super) fn checked_i64(value: u64, field: &'static str) -> Result<i64, MetricNormalizeError> {
    i64::try_from(value).map_err(|_| MetricNormalizeError::IntegerOverflow { field })
}
fn check_chars(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), MetricNormalizeError> {
    if value.chars().count() > maximum {
        return Err(MetricNormalizeError::FieldTooLong { field, maximum });
    }
    Ok(())
}
fn encode_json(value: Value, field: &'static str) -> Result<String, MetricNormalizeError> {
    let encoded = value.to_string();
    if encoded.len() > MAX_METADATA_JSON_BYTES {
        return Err(MetricNormalizeError::MetadataTooLarge {
            field,
            actual: encoded.len(),
            maximum: MAX_METADATA_JSON_BYTES,
        });
    }
    Ok(encoded)
}
fn nonempty_private(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| private_text(value))
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
