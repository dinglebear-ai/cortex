//! OTLP exponential histogram and summary normalization.

use super::super::metrics_payload::safe_double;
use super::{
    MetricNormalizeError, MetricPointInput, NumberMetricContext, PointParts, build_metric_point,
    validate_metric_envelope,
};
use crate::config::AgentObservatoryPrivacyConfig;
use opentelemetry_proto::tonic::{
    common::v1::InstrumentationScope,
    metrics::v1::{
        ExponentialHistogramDataPoint, Metric, SummaryDataPoint,
        exponential_histogram_data_point::Buckets, metric,
    },
    resource::v1::Resource,
};
use serde_json::{Value, json};

const MAX_DISTRIBUTION_VALUES: usize = 16_384;

#[cfg(test)]
pub(crate) fn normalize_distribution_metric(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    metric: &Metric,
    received_at: &str,
) -> Result<Vec<MetricPointInput>, MetricNormalizeError> {
    normalize_distribution_metric_with_privacy(
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
pub(crate) fn normalize_distribution_metric_with_privacy(
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
    metric: &Metric,
    privacy: &AgentObservatoryPrivacyConfig,
    received_at: &str,
) -> Result<Vec<MetricPointInput>, MetricNormalizeError> {
    validate_metric_envelope(resource, scope, metric, received_at)?;
    match metric.data.as_ref() {
        Some(metric::Data::ExponentialHistogram(histogram)) => {
            if histogram.aggregation_temporality == 0 {
                return Err(MetricNormalizeError::UnspecifiedTemporality);
            }
            let context = context(
                resource,
                resource_schema_url,
                scope,
                scope_schema_url,
                metric,
                privacy,
                received_at,
                "exponential_histogram",
                Some(histogram.aggregation_temporality),
            );
            histogram
                .data_points
                .iter()
                .map(|point| normalize_exponential_point(context, point))
                .collect()
        }
        Some(metric::Data::Summary(summary)) => {
            let context = context(
                resource,
                resource_schema_url,
                scope,
                scope_schema_url,
                metric,
                privacy,
                received_at,
                "summary",
                None,
            );
            summary
                .data_points
                .iter()
                .map(|point| normalize_summary_point(context, point))
                .collect()
        }
        _ => Err(MetricNormalizeError::UnsupportedInstrument),
    }
}

#[allow(clippy::too_many_arguments)]
fn context<'a>(
    resource: Option<&'a Resource>,
    resource_schema_url: &'a str,
    scope: Option<&'a InstrumentationScope>,
    scope_schema_url: &'a str,
    metric: &'a Metric,
    privacy: &'a AgentObservatoryPrivacyConfig,
    received_at: &'a str,
    instrument_kind: &'static str,
    aggregation_temporality: Option<i32>,
) -> NumberMetricContext<'a> {
    NumberMetricContext {
        resource,
        resource_schema_url,
        scope,
        scope_schema_url,
        metric,
        privacy,
        received_at,
        instrument_kind,
        aggregation_temporality,
        monotonic: None,
        ignore_start_time: false,
    }
}

fn normalize_exponential_point(
    context: NumberMetricContext<'_>,
    point: &ExponentialHistogramDataPoint,
) -> Result<MetricPointInput, MetricNormalizeError> {
    let positive = bucket_value(point.positive.as_ref())?;
    let negative = bucket_value(point.negative.as_ref())?;
    let bucket_total = bucket_total(point.positive.as_ref())?
        .checked_add(bucket_total(point.negative.as_ref())?)
        .and_then(|value| value.checked_add(point.zero_count));
    if bucket_total != Some(point.count) {
        return Err(MetricNormalizeError::ExponentialHistogramCountMismatch);
    }
    let value = json!({
        "count": point.count,
        "sum": point.sum.map(safe_double),
        "min": point.min.map(safe_double),
        "max": point.max.map(safe_double),
        "scale": point.scale,
        "zero_count": point.zero_count,
        "zero_threshold": safe_double(point.zero_threshold),
        "positive": positive,
        "negative": negative,
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

fn bucket_value(buckets: Option<&Buckets>) -> Result<Value, MetricNormalizeError> {
    let Some(buckets) = buckets else {
        return Ok(Value::Null);
    };
    check_value_limit(buckets.bucket_counts.len())?;
    Ok(json!({"offset": buckets.offset, "bucket_counts": buckets.bucket_counts}))
}

fn bucket_total(buckets: Option<&Buckets>) -> Result<u64, MetricNormalizeError> {
    buckets.map_or(Ok(0), |buckets| {
        buckets
            .bucket_counts
            .iter()
            .try_fold(0_u64, |total, count| {
                total
                    .checked_add(*count)
                    .ok_or(MetricNormalizeError::ExponentialHistogramCountMismatch)
            })
    })
}

fn normalize_summary_point(
    context: NumberMetricContext<'_>,
    point: &SummaryDataPoint,
) -> Result<MetricPointInput, MetricNormalizeError> {
    check_value_limit(point.quantile_values.len())?;
    let mut previous = None;
    let quantiles = point
        .quantile_values
        .iter()
        .map(|value| {
            if !value.quantile.is_finite()
                || !(0.0..=1.0).contains(&value.quantile)
                || previous.is_some_and(|prior| value.quantile <= prior)
            {
                return Err(MetricNormalizeError::InvalidSummaryQuantiles);
            }
            if !value.value.is_finite() {
                return Err(MetricNormalizeError::InvalidSummaryValue);
            }
            previous = Some(value.quantile);
            Ok(json!({"quantile": value.quantile, "value": value.value}))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let value = json!({
        "count": point.count,
        "sum": safe_double(point.sum),
        "quantile_values": quantiles,
        "flags": point.flags,
    });
    build_metric_point(
        context,
        PointParts {
            attributes: &point.attributes,
            exemplars: &[],
            start_time_unix_nano: point.start_time_unix_nano,
            time_unix_nano: point.time_unix_nano,
            value,
        },
    )
}

fn check_value_limit(actual: usize) -> Result<(), MetricNormalizeError> {
    if actual > MAX_DISTRIBUTION_VALUES {
        return Err(MetricNormalizeError::DistributionValueLimit {
            actual,
            maximum: MAX_DISTRIBUTION_VALUES,
        });
    }
    Ok(())
}
