use super::*;

use opentelemetry_proto::tonic::{
    common::v1::{AnyValue, EntityRef, KeyValue, any_value::Value as AnyValueKind},
    trace::v1::{Status, span, status},
};

const RECEIVED_AT: &str = "2026-08-18T20:15:00.000Z";

fn av(value: &str) -> AnyValue {
    AnyValue {
        value: Some(AnyValueKind::StringValue(value.to_string())),
    }
}

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(av(value)),
        key_strindex: 0,
    }
}

fn resource() -> Resource {
    Resource {
        attributes: vec![
            kv("host.name", "devhost"),
            kv("service.name", "claude-code"),
            kv("service.version", "1.2.3"),
            kv("session.id", "resource-session"),
            kv("project.path", "/resource/project"),
            kv("Authorization", "Bearer resource-secret"),
        ],
        dropped_attributes_count: 2,
        entity_refs: vec![EntityRef {
            schema_url: "https://opentelemetry.io/schemas/1.30.0".to_string(),
            r#type: "service".to_string(),
            id_keys: vec!["service.name".to_string()],
            description_keys: vec!["service.version".to_string()],
        }],
    }
}

fn scope() -> InstrumentationScope {
    InstrumentationScope {
        name: "cortex.trace.tests".to_string(),
        version: "0.1.0".to_string(),
        attributes: vec![kv("scope.custom", "kept"), kv("api_token", "scope-secret")],
        dropped_attributes_count: 1,
    }
}

fn span_fixture() -> Span {
    Span {
        trace_id: vec![0x11; 16],
        span_id: vec![0x22; 8],
        trace_state: "vendor=value".to_string(),
        parent_span_id: vec![0x33; 8],
        flags: 0x301,
        name: "tool.call".to_string(),
        kind: span::SpanKind::Client as i32,
        start_time_unix_nano: 1_700_000_000_000_000_000,
        end_time_unix_nano: 1_700_000_000_000_025_000,
        attributes: vec![
            kv("ai.tool", "codex"),
            kv("session_id", "span-session"),
            kv("gen_ai.conversation.id", "lower-priority-session"),
            kv("codebase.root_path", "/span/project"),
            kv("custom.span", "kept"),
            kv("Authorization", "Bearer span-secret"),
        ],
        dropped_attributes_count: 3,
        events: vec![],
        dropped_events_count: 4,
        links: vec![],
        dropped_links_count: 5,
        status: Some(Status {
            message: "boom".to_string(),
            code: status::StatusCode::Error as i32,
        }),
    }
}

#[test]
fn valid_span_normalizes_exact_ids_times_status_provider_and_resource_scope_context() {
    let resource = resource();
    let scope = scope();
    let span = span_fixture();
    let input = normalize_span(
        Some(&resource),
        "https://opentelemetry.io/schemas/1.30.0",
        Some(&scope),
        "https://opentelemetry.io/schemas/1.30.0",
        &span,
        RECEIVED_AT,
    )
    .unwrap();

    assert_eq!(input.trace_id, "11".repeat(16));
    assert_eq!(input.span_id, "22".repeat(8));
    assert_eq!(input.parent_span_id.as_deref(), Some("3333333333333333"));
    assert_eq!(input.trace_state.as_deref(), Some("vendor=value"));
    assert_eq!(input.flags, 0x301);
    assert_eq!(input.span_name, "tool.call");
    assert_eq!(input.span_kind, span::SpanKind::Client as i64);
    assert_eq!(input.start_time_unix_nano, 1_700_000_000_000_000_000);
    assert_eq!(input.end_time_unix_nano, 1_700_000_000_000_025_000);
    assert_eq!(input.duration_nano, 25_000);
    assert_eq!(input.status_code, status::StatusCode::Error as i64);
    assert_eq!(input.status_message.as_deref(), Some("boom"));
    assert_eq!(input.hostname, "devhost");
    assert_eq!(input.service_name.as_deref(), Some("claude-code"));
    assert_eq!(input.service_version.as_deref(), Some("1.2.3"));
    assert_eq!(input.scope_name.as_deref(), Some("cortex.trace.tests"));
    assert_eq!(input.scope_version.as_deref(), Some("0.1.0"));
    assert_eq!(input.ai_tool.as_deref(), Some("codex"));
    assert_eq!(input.ai_project.as_deref(), Some("/span/project"));
    assert_eq!(input.ai_session_id.as_deref(), Some("span-session"));
    assert_eq!(input.run_id, None);
    assert_eq!(input.events_json, "[]");
    assert_eq!(input.links_json, "[]");
    assert_eq!(input.received_at, RECEIVED_AT);
    assert!(!input.content_scrubbed);

    let attributes: serde_json::Value = serde_json::from_str(&input.attributes_json).unwrap();
    assert_eq!(attributes["custom.span"], "kept");
    assert_eq!(attributes["Authorization"], "[REDACTED]");

    let context: serde_json::Value = serde_json::from_str(&input.resource_json).unwrap();
    assert_eq!(context["resource"]["dropped_attributes_count"], 2);
    assert_eq!(
        context["resource"]["attributes"]["Authorization"],
        "[REDACTED]"
    );
    assert_eq!(context["resource"]["entity_refs"][0]["type"], "service");
    assert_eq!(context["scope"]["name"], "cortex.trace.tests");
    assert_eq!(context["scope"]["attributes"]["scope.custom"], "kept");
    assert_eq!(context["scope"]["attributes"]["api_token"], "[REDACTED]");
    assert_eq!(context["scope"]["dropped_attributes_count"], 1);
}

#[test]
fn unknown_future_span_kind_and_status_code_are_preserved_as_raw_integers() {
    let resource = resource();
    let mut span = span_fixture();
    span.kind = 99;
    span.status = Some(Status {
        message: "future status".to_string(),
        code: 77,
    });

    let input = normalize_span(Some(&resource), "", None, "", &span, RECEIVED_AT).unwrap();
    assert_eq!(input.span_kind, 99);
    assert_eq!(input.status_code, 77);
    assert_eq!(input.status_message.as_deref(), Some("future status"));
}

#[test]
fn root_span_preserves_absent_optional_parent_trace_state_status_and_scope() {
    let resource = resource();
    let mut span = span_fixture();
    span.parent_span_id.clear();
    span.trace_state.clear();
    span.status = None;
    let input = normalize_span(Some(&resource), "", None, "", &span, RECEIVED_AT).unwrap();

    assert_eq!(input.parent_span_id, None);
    assert_eq!(input.trace_state, None);
    assert_eq!(input.status_code, 0);
    assert_eq!(input.status_message, None);
    assert_eq!(input.scope_name, None);
    assert_eq!(input.scope_version, None);
}

#[test]
fn zero_or_wrong_length_trace_span_and_parent_ids_are_rejected() {
    let resource = resource();
    let scope = scope();
    let cases = [
        ("trace-empty", vec![], vec![0x22; 8], vec![0x33; 8]),
        ("trace-short", vec![1; 15], vec![0x22; 8], vec![0x33; 8]),
        ("trace-zero", vec![0; 16], vec![0x22; 8], vec![0x33; 8]),
        ("span-short", vec![0x11; 16], vec![2; 7], vec![0x33; 8]),
        ("span-zero", vec![0x11; 16], vec![0; 8], vec![0x33; 8]),
        ("parent-short", vec![0x11; 16], vec![0x22; 8], vec![3; 7]),
        ("parent-zero", vec![0x11; 16], vec![0x22; 8], vec![0; 8]),
    ];

    for (name, trace_id, span_id, parent_span_id) in cases {
        let mut span = span_fixture();
        span.trace_id = trace_id;
        span.span_id = span_id;
        span.parent_span_id = parent_span_id;
        let error =
            normalize_span(Some(&resource), "", Some(&scope), "", &span, RECEIVED_AT).unwrap_err();
        assert!(
            matches!(error, TraceNormalizeError::InvalidId { .. }),
            "{name}: {error}"
        );
    }
}

#[test]
fn time_order_and_sqlite_integer_overflow_are_rejected() {
    let resource = resource();
    let mut span = span_fixture();
    span.end_time_unix_nano = span.start_time_unix_nano - 1;
    assert_eq!(
        normalize_span(Some(&resource), "", None, "", &span, RECEIVED_AT).unwrap_err(),
        TraceNormalizeError::EndBeforeStart
    );

    let mut span = span_fixture();
    span.start_time_unix_nano = u64::MAX;
    span.end_time_unix_nano = u64::MAX;
    assert!(matches!(
        normalize_span(Some(&resource), "", None, "", &span, RECEIVED_AT).unwrap_err(),
        TraceNormalizeError::IntegerOverflow {
            field: "start_time_unix_nano"
        }
    ));
}

#[test]
fn resource_scope_and_span_attribute_caps_are_typed_errors() {
    let mut oversized_resource = resource();
    oversized_resource.attributes = (0..=MAX_RESOURCE_ATTRIBUTES)
        .map(|index| kv(&format!("resource.{index}"), "value"))
        .collect();
    let span = span_fixture();
    assert!(matches!(
        normalize_span(Some(&oversized_resource), "", None, "", &span, RECEIVED_AT).unwrap_err(),
        TraceNormalizeError::AttributeLimit {
            field: "resource",
            ..
        }
    ));

    let valid_resource = resource();
    let mut scope = scope();
    scope.attributes = (0..=MAX_RESOURCE_ATTRIBUTES)
        .map(|index| kv(&format!("scope.{index}"), "value"))
        .collect();
    assert!(matches!(
        normalize_span(
            Some(&valid_resource),
            "",
            Some(&scope),
            "",
            &span,
            RECEIVED_AT
        )
        .unwrap_err(),
        TraceNormalizeError::AttributeLimit { field: "scope", .. }
    ));

    let mut span = span_fixture();
    span.attributes = (0..=MAX_SIGNAL_ATTRIBUTES)
        .map(|index| kv(&format!("span.{index}"), "value"))
        .collect();
    assert!(matches!(
        normalize_span(Some(&valid_resource), "", None, "", &span, RECEIVED_AT).unwrap_err(),
        TraceNormalizeError::AttributeLimit { field: "span", .. }
    ));
}

#[test]
fn field_received_at_and_serialized_metadata_limits_are_enforced() {
    let valid_resource = resource();
    let mut span = span_fixture();
    span.name = "n".repeat(MAX_SPAN_NAME_CHARS + 1);
    assert!(matches!(
        normalize_span(Some(&valid_resource), "", None, "", &span, RECEIVED_AT).unwrap_err(),
        TraceNormalizeError::FieldTooLong {
            field: "span_name",
            ..
        }
    ));

    let mut long_service = resource();
    long_service
        .attributes
        .retain(|attr| attr.key != "service.name");
    long_service
        .attributes
        .push(kv("service.name", &"s".repeat(MAX_SERVICE_NAME_CHARS + 1)));
    assert!(matches!(
        normalize_span(
            Some(&long_service),
            "",
            None,
            "",
            &span_fixture(),
            RECEIVED_AT
        )
        .unwrap_err(),
        TraceNormalizeError::FieldTooLong {
            field: "service_name",
            ..
        }
    ));

    let mut span = span_fixture();
    span.status = Some(Status {
        message: "m".repeat(MAX_STATUS_MESSAGE_CHARS + 1),
        code: status::StatusCode::Error as i32,
    });
    assert!(matches!(
        normalize_span(Some(&valid_resource), "", None, "", &span, RECEIVED_AT).unwrap_err(),
        TraceNormalizeError::FieldTooLong {
            field: "status_message",
            ..
        }
    ));

    assert_eq!(
        normalize_span(
            Some(&valid_resource),
            "",
            None,
            "",
            &span_fixture(),
            "not-a-time"
        )
        .unwrap_err(),
        TraceNormalizeError::InvalidReceivedAt
    );

    let mut span = span_fixture();
    span.attributes = (0..MAX_SIGNAL_ATTRIBUTES)
        .map(|index| kv(&format!("span.{index:03}"), &"x".repeat(4096)))
        .collect();
    assert!(matches!(
        normalize_span(Some(&valid_resource), "", None, "", &span, RECEIVED_AT).unwrap_err(),
        TraceNormalizeError::MetadataTooLarge {
            field: "attributes",
            ..
        }
    ));
}
