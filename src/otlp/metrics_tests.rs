use super::*;
use opentelemetry_proto::tonic::{
    common::v1::{
        AnyValue, EntityRef, InstrumentationScope, KeyValue, any_value::Value as AnyValueKind,
    },
    metrics::v1::{
        AggregationTemporality, Exemplar, Gauge, Sum, exemplar, metric, number_data_point,
    },
    resource::v1::Resource,
};

const RECEIVED_AT: &str = "2026-08-19T18:00:00.000Z";

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(AnyValueKind::StringValue(value.to_string())),
        }),
        key_strindex: 0,
    }
}
fn resource(attrs: Vec<KeyValue>) -> Resource {
    Resource {
        attributes: attrs,
        ..Default::default()
    }
}
fn scope() -> InstrumentationScope {
    InstrumentationScope {
        name: "agent.metrics".into(),
        version: "1.2.3".into(),
        ..Default::default()
    }
}
fn number_point(value: number_data_point::Value) -> NumberDataPoint {
    NumberDataPoint {
        attributes: vec![kv("z", "last"), kv("a", "first")],
        start_time_unix_nano: 100,
        time_unix_nano: 200,
        value: Some(value),
        ..Default::default()
    }
}
fn gauge(point: NumberDataPoint) -> Metric {
    Metric {
        name: "agent.queue.depth".into(),
        description: "queued work".into(),
        unit: "{item}".into(),
        data: Some(metric::Data::Gauge(Gauge {
            data_points: vec![point],
        })),
        ..Default::default()
    }
}
fn sum(point: NumberDataPoint, temporality: AggregationTemporality, monotonic: bool) -> Metric {
    Metric {
        name: "agent.tokens".into(),
        description: "tokens".into(),
        unit: "{token}".into(),
        data: Some(metric::Data::Sum(Sum {
            data_points: vec![point],
            aggregation_temporality: temporality as i32,
            is_monotonic: monotonic,
        })),
        ..Default::default()
    }
}
fn normalize(metric: &Metric, resource: &Resource) -> MetricPointInput {
    let scope = scope();
    normalize_number_metric(
        Some(resource),
        "resource/v1",
        Some(&scope),
        "scope/v1",
        metric,
        RECEIVED_AT,
    )
    .unwrap()
    .into_iter()
    .next()
    .unwrap()
}

#[test]
fn integer_gauge_ignores_start_time_and_normalizes_identity() {
    let resource = resource(vec![
        kv("service.name", "claude-code"),
        kv("host.name", "dookie"),
    ]);
    let output = normalize(
        &gauge(number_point(number_data_point::Value::AsInt(7))),
        &resource,
    );
    assert_eq!(output.instrument_kind, "gauge");
    assert_eq!(output.start_time_unix_nano, None);
    assert_eq!(output.aggregation_temporality, None);
    assert_eq!(output.monotonic, None);
    assert_eq!(output.time_unix_nano, 200);
    assert_eq!(output.hostname, "dookie");
    assert_eq!(output.ai_tool.as_deref(), Some("claude"));
    assert_eq!(
        serde_json::from_str::<Value>(&output.value_json).unwrap(),
        json!({"type":"int","value":7,"flags":0})
    );
}

#[test]
fn cumulative_and_delta_sums_preserve_temporality_monotonic_and_start() {
    let resource = resource(Vec::new());
    for (temporality, monotonic, value) in [
        (
            AggregationTemporality::Cumulative,
            true,
            number_data_point::Value::AsInt(42),
        ),
        (
            AggregationTemporality::Delta,
            false,
            number_data_point::Value::AsDouble(2.5),
        ),
    ] {
        let output = normalize(&sum(number_point(value), temporality, monotonic), &resource);
        assert_eq!(output.instrument_kind, "sum");
        assert_eq!(output.aggregation_temporality, Some(temporality as i32));
        assert_eq!(output.monotonic, Some(monotonic));
        assert_eq!(output.start_time_unix_nano, Some(100));
    }
}

#[test]
fn repeated_fixture_and_reordered_attributes_have_same_point_key() {
    let mut resource = resource(vec![kv("service.name", "codex"), kv("host.name", "dookie")]);
    resource.entity_refs = vec![
        EntityRef {
            schema_url: "entity/v1".into(),
            r#type: "service".into(),
            id_keys: vec!["service.name".into(), "host.name".into()],
            description_keys: vec!["host.name".into()],
        },
        EntityRef {
            schema_url: "entity/v1".into(),
            r#type: "host".into(),
            id_keys: vec!["host.name".into()],
            description_keys: Vec::new(),
        },
    ];
    let metric = gauge(number_point(number_data_point::Value::AsInt(7)));
    let first = normalize(&metric, &resource);
    let second = normalize(&metric, &resource);
    assert_eq!(first.point_key, second.point_key);

    let mut reordered_point = number_point(number_data_point::Value::AsInt(7));
    reordered_point.attributes.reverse();
    let third = normalize(&gauge(reordered_point), &resource);
    assert_eq!(first.point_key, third.point_key);
    assert_eq!(first.attributes_json, third.attributes_json);

    let mut reordered_resource = resource.clone();
    reordered_resource.attributes.reverse();
    reordered_resource.entity_refs.reverse();
    reordered_resource.entity_refs[1].id_keys.reverse();
    let fourth = normalize(&metric, &reordered_resource);
    assert_eq!(first.point_key, fourth.point_key);
    assert_eq!(first.resource_json, fourth.resource_json);
}

#[test]
fn point_key_separates_identifying_stream_properties_and_flags() {
    let resource = resource(Vec::new());
    let point = number_point(number_data_point::Value::AsInt(7));
    let base_metric = sum(point.clone(), AggregationTemporality::Cumulative, true);
    let base = normalize(&base_metric, &resource);

    let mut changed_description = base_metric.clone();
    changed_description.description = "documentation only".into();
    assert_eq!(
        base.point_key,
        normalize(&changed_description, &resource).point_key
    );

    let mut changed_unit = base_metric.clone();
    changed_unit.unit = "ms".into();
    assert_ne!(
        base.point_key,
        normalize(&changed_unit, &resource).point_key
    );

    let changed_temporality = normalize(
        &sum(point.clone(), AggregationTemporality::Delta, true),
        &resource,
    );
    assert_ne!(base.point_key, changed_temporality.point_key);

    let changed_monotonic = normalize(
        &sum(point.clone(), AggregationTemporality::Cumulative, false),
        &resource,
    );
    assert_ne!(base.point_key, changed_monotonic.point_key);

    let mut flagged_point = point;
    flagged_point.flags = 1;
    let flagged = normalize(
        &sum(flagged_point, AggregationTemporality::Cumulative, true),
        &resource,
    );
    assert_ne!(base.point_key, flagged.point_key);
    let value: Value = serde_json::from_str(&flagged.value_json).unwrap();
    assert_eq!(value["flags"], 1);
}

#[test]
fn non_finite_double_values_are_lossless_valid_json_tokens() {
    let resource = resource(Vec::new());
    for (value, expected) in [
        (f64::NAN, "nan"),
        (f64::INFINITY, "+infinity"),
        (f64::NEG_INFINITY, "-infinity"),
    ] {
        let output = normalize(
            &gauge(number_point(number_data_point::Value::AsDouble(value))),
            &resource,
        );
        let parsed: Value = serde_json::from_str(&output.value_json).unwrap();
        assert_eq!(parsed["value"], expected);
    }
}

#[test]
fn exemplar_ids_are_validated_serialized_and_affect_point_key() {
    let resource = resource(Vec::new());
    let mut point = number_point(number_data_point::Value::AsInt(7));
    point.exemplars.push(Exemplar {
        filtered_attributes: vec![kv("user.email", "alice@example.invalid")],
        time_unix_nano: 150,
        span_id: vec![0x22; 8],
        trace_id: vec![0x11; 16],
        value: Some(exemplar::Value::AsDouble(1.5)),
    });
    let with_exemplar = normalize(&gauge(point.clone()), &resource);
    let exemplars: Value = serde_json::from_str(&with_exemplar.exemplars_json).unwrap();
    assert_eq!(exemplars[0]["trace_id"], "11111111111111111111111111111111");
    assert!(
        exemplars[0]["filtered_attributes"]["user.email"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    point.exemplars[0].span_id = vec![0x33; 8];
    let changed = normalize(&gauge(point), &resource);
    assert_ne!(with_exemplar.point_key, changed.point_key);
}

#[test]
fn invalid_or_missing_number_point_fields_fail_closed() {
    let resource = resource(Vec::new());
    let scope = scope();
    let mut missing = number_point(number_data_point::Value::AsInt(1));
    missing.value = None;
    assert_eq!(
        normalize_number_metric(
            Some(&resource),
            "",
            Some(&scope),
            "",
            &gauge(missing),
            RECEIVED_AT
        )
        .unwrap_err(),
        MetricNormalizeError::MissingValue
    );
    let mut zero_time = number_point(number_data_point::Value::AsInt(1));
    zero_time.time_unix_nano = 0;
    assert_eq!(
        normalize_number_metric(
            Some(&resource),
            "",
            Some(&scope),
            "",
            &gauge(zero_time),
            RECEIVED_AT
        )
        .unwrap_err(),
        MetricNormalizeError::MissingPointTime
    );
    let unspecified = sum(
        number_point(number_data_point::Value::AsInt(1)),
        AggregationTemporality::Unspecified,
        true,
    );
    assert_eq!(
        normalize_number_metric(
            Some(&resource),
            "",
            Some(&scope),
            "",
            &unspecified,
            RECEIVED_AT
        )
        .unwrap_err(),
        MetricNormalizeError::UnspecifiedTemporality
    );
}

#[test]
fn metric_attributes_follow_configured_privacy_policy() {
    let resource = resource(vec![kv("project.path", "/secret/project")]);
    let mut point = number_point(number_data_point::Value::AsInt(1));
    point.attributes.push(kv("gen_ai.prompt", "secret prompt"));
    let metric = gauge(point);

    let default_output = normalize(&metric, &resource);
    let attrs: Value = serde_json::from_str(&default_output.attributes_json).unwrap();
    assert_eq!(attrs["gen_ai.prompt"], "[REDACTED]");
    assert_eq!(
        default_output.ai_project.as_deref(),
        Some("/secret/project")
    );

    let privacy = AgentObservatoryPrivacyConfig {
        include_paths: false,
        ..Default::default()
    };
    let scope = scope();
    let private_output = normalize_number_metric_with_privacy(
        Some(&resource),
        "resource/v1",
        Some(&scope),
        "scope/v1",
        &metric,
        &privacy,
        RECEIVED_AT,
    )
    .unwrap()
    .into_iter()
    .next()
    .unwrap();
    assert_eq!(private_output.ai_project, None);
    assert!(!private_output.resource_json.contains("/secret/project"));
}
