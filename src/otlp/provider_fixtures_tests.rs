use std::{collections::BTreeMap, net::SocketAddr};

use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value::Value as AnyValueKind},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    metrics::v1::{Gauge, Metric, NumberDataPoint, metric, number_data_point},
    resource::v1::Resource,
    trace::v1::{Span, Status},
};
use serde::Deserialize;

use super::{entries::build_entries, metrics::normalize_number_metric, traces::normalize_span};

const RECEIVED_AT: &str = "2026-08-21T12:00:00Z";

#[derive(Debug, Deserialize)]
struct Fixture {
    provider: String,
    provenance: Provenance,
    resource: BTreeMap<String, String>,
    log: Option<LogFixture>,
    span: Option<SpanFixture>,
    metric: Option<MetricFixture>,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Provenance {
    url: String,
    retrieved: String,
    note: String,
}

#[derive(Debug, Deserialize)]
struct LogFixture {
    event_name: String,
    body: String,
    tool_name: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct SpanFixture {
    name: String,
    tool_name: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct MetricFixture {
    name: String,
    value: i64,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct Expected {
    tool: String,
    session_id: String,
    project: String,
    log: String,
    trace: String,
    metric: String,
}

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(AnyValueKind::StringValue(value.into())),
        }),
        key_strindex: 0,
    }
}

fn fixture(name: &str) -> Fixture {
    let raw = match name {
        "claude" => include_str!("../../tests/fixtures/otlp/claude.json"),
        "codex" => include_str!("../../tests/fixtures/otlp/codex.json"),
        "gemini" => include_str!("../../tests/fixtures/otlp/gemini.json"),
        _ => unreachable!(),
    };
    serde_json::from_str(raw).unwrap()
}

fn resource(fixture: &Fixture) -> Resource {
    Resource {
        attributes: fixture
            .resource
            .iter()
            .map(|(key, value)| kv(key, value))
            .collect(),
        ..Default::default()
    }
}

fn assert_common_fixture_contract(fixture: &Fixture) {
    assert_eq!(fixture.provider, fixture.expected.tool);
    assert!(fixture.provenance.url.starts_with("https://"));
    assert_eq!(fixture.provenance.retrieved, "2026-08-21");
    assert!(fixture.provenance.note.contains("Synthetic") || fixture.provider == "codex");
    assert_eq!(
        fixture.expected.log,
        if fixture.log.is_some() {
            "observed"
        } else {
            "not_observed"
        }
    );
    assert_eq!(
        fixture.expected.trace,
        if fixture.span.is_some() {
            "observed"
        } else {
            "not_observed"
        }
    );
    assert_eq!(
        fixture.expected.metric,
        if fixture.metric.is_some() {
            "observed"
        } else {
            "not_observed"
        }
    );
}

fn assert_log(fixture: &Fixture, resource: &Resource) {
    let Some(log) = &fixture.log else { return };
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource.clone()),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: format!("{}.telemetry", fixture.provider),
                    ..Default::default()
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: 1_777_000_000_000_000_000,
                    body: Some(AnyValue {
                        value: Some(AnyValueKind::StringValue(log.body.clone())),
                    }),
                    event_name: log.event_name.clone(),
                    attributes: vec![
                        kv("gen_ai.tool.name", &log.tool_name),
                        kv("gen_ai.prompt", &log.prompt),
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let entries = build_entries(&request, "127.0.0.1:4318".parse::<SocketAddr>().unwrap());
    let entry = entries.first().unwrap();
    // The legacy log converter intentionally leaves service-name aliasing to
    // downstream enrichment; traces and metrics use the full normalizer here.
    assert_eq!(entry.ai_tool, None);
    assert_eq!(
        entry.app_name.as_deref(),
        fixture.resource.get("service.name").map(String::as_str)
    );
    assert_eq!(
        entry.ai_session_id.as_deref(),
        Some(fixture.expected.session_id.as_str())
    );
    assert_eq!(
        entry.ai_project.as_deref(),
        Some(fixture.expected.project.as_str())
    );
}

fn assert_span(fixture: &Fixture, resource: &Resource) {
    let Some(input) = &fixture.span else { return };
    let span = Span {
        trace_id: vec![1; 16],
        span_id: vec![2; 8],
        name: input.name.clone(),
        start_time_unix_nano: 100,
        end_time_unix_nano: 200,
        attributes: vec![
            kv("gen_ai.tool.name", &input.tool_name),
            kv("gen_ai.input.messages", &input.prompt),
        ],
        status: Some(Status::default()),
        ..Default::default()
    };
    let output = normalize_span(Some(resource), "", None, "", &span, RECEIVED_AT).unwrap();
    assert_eq!(
        output.ai_tool.as_deref(),
        Some(fixture.expected.tool.as_str())
    );
    assert_eq!(
        output.ai_session_id.as_deref(),
        Some(fixture.expected.session_id.as_str())
    );
    assert_eq!(
        output.ai_project.as_deref(),
        Some(fixture.expected.project.as_str())
    );
    assert!(!output.attributes_json.contains(&input.prompt));
    assert!(output.attributes_json.contains("[REDACTED]"));
}

fn assert_metric(fixture: &Fixture, resource: &Resource) {
    let Some(input) = &fixture.metric else { return };
    let metric = Metric {
        name: input.name.clone(),
        data: Some(metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                time_unix_nano: 200,
                value: Some(number_data_point::Value::AsInt(input.value)),
                attributes: vec![kv("gen_ai.prompt", &input.prompt)],
                ..Default::default()
            }],
        })),
        ..Default::default()
    };
    let output = normalize_number_metric(Some(resource), "", None, "", &metric, RECEIVED_AT)
        .unwrap()
        .remove(0);
    assert_eq!(
        output.ai_tool.as_deref(),
        Some(fixture.expected.tool.as_str())
    );
    assert_eq!(
        output.ai_session_id.as_deref(),
        Some(fixture.expected.session_id.as_str())
    );
    assert_eq!(
        output.ai_project.as_deref(),
        Some(fixture.expected.project.as_str())
    );
    assert!(!output.attributes_json.contains(&input.prompt));
    assert!(output.attributes_json.contains("[REDACTED]"));
}

#[test]
fn official_shape_provider_fixtures_normalize_identity_signals_and_privacy() {
    for name in ["claude", "codex", "gemini"] {
        let fixture = fixture(name);
        assert_common_fixture_contract(&fixture);
        let resource = resource(&fixture);
        assert_log(&fixture, &resource);
        assert_span(&fixture, &resource);
        assert_metric(&fixture, &resource);
    }
}

#[test]
fn gemini_preserves_both_session_identifiers_and_contract_precedence() {
    let fixture = fixture("gemini");
    let resource = resource(&fixture);
    let conversation_id = fixture.resource.get("gen_ai.conversation.id").unwrap();
    let span = fixture.span.as_ref().unwrap();
    let normalized = normalize_span(
        Some(&resource),
        "",
        None,
        "",
        &Span {
            trace_id: vec![3; 16],
            span_id: vec![4; 8],
            name: span.name.clone(),
            start_time_unix_nano: 100,
            end_time_unix_nano: 200,
            ..Default::default()
        },
        RECEIVED_AT,
    )
    .unwrap();
    assert_eq!(
        normalized.ai_session_id.as_deref(),
        Some(fixture.expected.session_id.as_str())
    );
    assert!(normalized.resource_json.contains(conversation_id));
    assert!(
        normalized
            .resource_json
            .contains(&fixture.expected.session_id)
    );
}
