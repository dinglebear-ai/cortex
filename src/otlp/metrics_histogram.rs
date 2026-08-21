//! OTLP explicit histogram normalization for Agent Observatory metric points.

use super::{
    MetricNormalizeError, MetricPointInput, NumberMetricContext, PointParts, build_metric_point,
    validate_metric_envelope,
};
use crate::config::AgentObservatoryPrivacyConfig;
use opentelemetry_proto::tonic::{
    common::v1::InstrumentationScope,
    metrics::v1::{HistogramDataPoint, Metric, metric},
    resource::v1::Resource,
};
use serde_json::{Value, json};

const MAX_HISTOGRAM_BUCKETS: usize = 16_384;

#[cfg(test)]
pub(crate) fn normalize_histogram_metric(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    metric: &Metric,
    received_at: &str,
) -> Result<Vec<MetricPointInput>, MetricNormalizeError> {
    normalize_histogram_metric_with_privacy(
        resource,
        resource_schema_url,
        scope,
        scope_schema_url,
        metric,
        &AgentObservatoryPrivacyConfig::default(),
        received_at,
    )
}

#[allow(dead_code)]
pub(crate) fn normalize_histogram_metric_with_privacy(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    metric: &Metric,
    privacy: &AgentObservatoryPrivacyConfig,
    received_at: &str,
) -> Result<Vec<MetricPointInput>, MetricNormalizeError> {
    validate_metric_envelope(resource, scope, metric, received_at)?;
    let histogram = match metric.data.as_ref() {
        Some(metric::Data::Histogram(value)) => value,
        _ => return Err(MetricNormalizeError::UnsupportedInstrument),
    };
    if histogram.aggregation_temporality == 0 {
        return Err(MetricNormalizeError::UnspecifiedTemporality);
    }
    let context = NumberMetricContext {
        resource,
        resource_schema_url,
        scope,
        scope_schema_url,
        metric,
        privacy,
        received_at,
        instrument_kind: "histogram",
        aggregation_temporality: Some(histogram.aggregation_temporality),
        monotonic: None,
        ignore_start_time: false,
    };
    histogram
        .data_points
        .iter()
        .map(|point| normalize_histogram_point(context, point))
        .collect()
}

fn normalize_histogram_point(
    context: NumberMetricContext<'_>,
    point: &HistogramDataPoint,
) -> Result<MetricPointInput, MetricNormalizeError> {
    validate_buckets(point)?;
    let value = json!({
        "count": point.count,
        "sum": point.sum.map(safe_double),
        "min": point.min.map(safe_double),
        "max": point.max.map(safe_double),
        "explicit_bounds": point.explicit_bounds.iter().copied().map(safe_double).collect::<Vec<_>>(),
        "bucket_counts": point.bucket_counts,
        "flags": point.flags,
    });
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

fn validate_buckets(point: &HistogramDataPoint) -> Result<(), MetricNormalizeError> {
    if point.bucket_counts.len() > MAX_HISTOGRAM_BUCKETS {
        return Err(MetricNormalizeError::HistogramBucketLimit {
            actual: point.bucket_counts.len(),
            maximum: MAX_HISTOGRAM_BUCKETS,
        });
    }
    let shape_valid = if point.bucket_counts.is_empty() {
        point.explicit_bounds.is_empty()
    } else {
        point.bucket_counts.len() == point.explicit_bounds.len() + 1
    };
    if !shape_valid {
        return Err(MetricNormalizeError::InvalidHistogramShape {
            buckets: point.bucket_counts.len(),
            bounds: point.explicit_bounds.len(),
        });
    }
    if point
        .bucket_counts
        .iter()
        .try_fold(0_u64, |total, count| total.checked_add(*count))
        != Some(point.count)
    {
        return Err(MetricNormalizeError::HistogramCountMismatch);
    }
    if !point
        .explicit_bounds
        .windows(2)
        .all(|pair| pair[0] < pair[1])
        || point.explicit_bounds.iter().any(|bound| !bound.is_finite())
    {
        return Err(MetricNormalizeError::HistogramBoundsNotIncreasing);
    }
    Ok(())
}

fn safe_double(value: f64) -> Value {
    if value.is_nan() {
        Value::String("nan".to_string())
    } else if value == f64::INFINITY {
        Value::String("+infinity".to_string())
    } else if value == f64::NEG_INFINITY {
        Value::String("-infinity".to_string())
    } else {
        serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
    }
}
