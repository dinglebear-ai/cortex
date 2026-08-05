//! Tests for OTLP → `LogBatchEntry` conversion and `AnyValue` extraction.

use super::*;

use opentelemetry_proto::tonic::{
    common::v1::KeyValue,
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    resource::v1::Resource,
};

fn av_string(s: &str) -> AnyValue {
    AnyValue {
        value: Some(AnyValueKind::StringValue(s.to_string())),
    }
}

fn kv(key: &str, value: AnyValue) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(value),
        // String-table-indexed key encoding is a wire-compatible OTLP
        // extension cortex doesn't emit/consume; 0 means "not indexed".
        key_strindex: 0,
    }
}

#[test]
fn any_value_to_json_renders_string_table_index_as_placeholder() {
    let v = AnyValue {
        value: Some(AnyValueKind::StringValueStrindex(7)),
    };
    assert_eq!(
        any_value_to_json(&v),
        serde_json::json!({"string_table_index": 7})
    );
}

#[test]
fn any_value_to_string_renders_string_table_index_as_placeholder() {
    let v = AnyValue {
        value: Some(AnyValueKind::StringValueStrindex(7)),
    };
    assert_eq!(
        any_value_to_string(&v),
        Some("[string_table_index=7]".to_string())
    );
}

// Exercises the pure gating function directly (not the global
// LAST_KEY_STRINDEX_WARNING / LAST_VALUE_STRINDEX_WARNING statics) so this
// test can't interfere with or be affected by other tests sharing the
// process-wide limiters -- same isolation approach as otlp::auth's
// `unauthorized_warning_rate_limit_suppresses_repeats_per_key`.
#[test]
fn string_table_index_warning_suppresses_repeats_within_interval() {
    let mut last = None;
    let now = std::time::Instant::now();
    let interval = std::time::Duration::from_secs(60);

    assert!(record_string_table_index_warning(&mut last, now, interval));
    assert!(!record_string_table_index_warning(
        &mut last,
        now + std::time::Duration::from_secs(30),
        interval,
    ));
    assert!(record_string_table_index_warning(
        &mut last,
        now + interval,
        interval,
    ));
}

fn sample_request(
    host: &str,
    service: &str,
    body: &str,
    severity: i32,
) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    kv("host.name", av_string(host)),
                    kv("service.name", av_string(service)),
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 1_700_000_000_000_000_000,
                    observed_time_unix_nano: 0,
                    severity_number: severity,
                    severity_text: String::new(),
                    body: Some(av_string(body)),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

// ---- severity mapping ----

#[test]
fn severity_unspecified_falls_back_to_info() {
    assert_eq!(severity_from_number(0), "info");
    assert_eq!(severity_from_number(99), "info");
    assert_eq!(severity_from_number(-1), "info");
}

#[test]
fn severity_buckets_match_otlp_spec() {
    assert_eq!(severity_from_number(1), "debug"); // TRACE
    assert_eq!(severity_from_number(5), "debug"); // DEBUG
    assert_eq!(severity_from_number(9), "info"); // INFO
    assert_eq!(severity_from_number(13), "warning"); // WARN
    assert_eq!(severity_from_number(17), "err"); // ERROR
    assert_eq!(severity_from_number(21), "crit"); // FATAL
}

// ---- AnyValue extraction ----

#[test]
fn any_value_string() {
    assert_eq!(any_value_to_string(&av_string("hi")).as_deref(), Some("hi"));
}

#[test]
fn any_value_none_returns_none() {
    let v = AnyValue { value: None };
    assert!(any_value_to_string(&v).is_none());
}

#[test]
fn any_value_int_and_bool_stringify() {
    let int_val = AnyValue {
        value: Some(AnyValueKind::IntValue(42)),
    };
    assert_eq!(any_value_to_string(&int_val).as_deref(), Some("42"));
    let bool_val = AnyValue {
        value: Some(AnyValueKind::BoolValue(true)),
    };
    assert_eq!(any_value_to_string(&bool_val).as_deref(), Some("true"));
}

// ---- build_entries ----

#[test]
fn build_entries_extracts_resource_attrs() {
    let peer = "127.0.0.1:12345".parse().unwrap();
    let req = sample_request("devhost", "claude-code", "tool_call started", 9);
    let entries = build_entries(&req, peer);
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.hostname, "devhost");
    assert_eq!(e.app_name.as_deref(), Some("claude-code"));
    assert_eq!(e.message, "tool_call started");
    assert_eq!(e.severity, "info");
    assert_eq!(e.facility.as_deref(), Some("otlp"));
    assert_eq!(e.source_ip, "127.0.0.1:12345");
    let metadata: serde_json::Value =
        serde_json::from_str(e.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(metadata["source_type"], "otlp");
    assert_eq!(metadata["peer_ip"], "127.0.0.1");
    assert_eq!(metadata["peer_port"], 12345);
    assert_eq!(metadata["service_name"], "claude-code");
    assert_eq!(metadata["resource_attributes"]["host.name"], "devhost");
}

#[test]
fn build_entries_missing_host_name_uses_empty_string() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: Some(av_string("orphan")),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let entries = build_entries(&req, peer);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hostname, "");
    assert!(entries[0].app_name.is_none());
}

#[test]
fn build_entries_no_panic_on_empty_body() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: None,
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: None,
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let entries = build_entries(&req, peer);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "");
}

#[test]
fn build_entries_handles_multiple_resource_logs() {
    let peer = "10.0.0.1:9999".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![
            sample_request("devhost", "claude-code", "msg one", 9)
                .resource_logs
                .into_iter()
                .next()
                .unwrap(),
            sample_request("edgehost", "codex", "msg two", 17)
                .resource_logs
                .into_iter()
                .next()
                .unwrap(),
        ],
    };
    let entries = build_entries(&req, peer);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].hostname, "devhost");
    assert_eq!(entries[1].hostname, "edgehost");
    assert_eq!(entries[1].severity, "err");
}

#[test]
fn build_entries_extracts_ai_metadata_from_attributes() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    kv("host.name", av_string("nashost")),
                    kv("service.name", av_string("claude-code")),
                    kv("session.id", av_string("res-session-123")),
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: Some(av_string("msg with log-level attributes")),
                    attributes: vec![
                        kv("session.id", av_string("log-session-456")), // overrides resource
                        kv("project.path", av_string("/work/cortex")),
                        kv("Authorization", av_string("Bearer secret")),
                    ],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let entries = build_entries(&req, peer);
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.hostname, "nashost");
    assert_eq!(e.app_name.as_deref(), Some("claude-code"));
    assert_eq!(e.ai_tool, None);
    assert_eq!(e.ai_session_id.as_deref(), Some("log-session-456"));
    assert_eq!(e.ai_project.as_deref(), Some("/work/cortex"));

    let metadata: serde_json::Value =
        serde_json::from_str(e.metadata_json.as_deref().unwrap()).unwrap();
    assert_eq!(metadata["log_attributes"]["Authorization"], "[REDACTED]");
}

#[test]
fn build_entries_extracts_ai_tool_from_explicit_attribute() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("host.name", av_string("nashost"))],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: Some(av_string("msg")),
                    attributes: vec![kv("ai.tool", av_string("claude"))],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let entries = build_entries(&req, peer);
    assert_eq!(entries[0].ai_tool.as_deref(), Some("claude"));
}

#[test]
fn build_entries_ignores_unknown_or_oversized_ai_tool() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("host.name", av_string("nashost"))],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![
                    LogRecord {
                        time_unix_nano: 0,
                        observed_time_unix_nano: 0,
                        severity_number: 9,
                        severity_text: String::new(),
                        body: Some(av_string("msg")),
                        attributes: vec![kv("ai.tool", av_string("unknown"))],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: vec![],
                        span_id: vec![],
                        event_name: String::new(),
                    },
                    LogRecord {
                        time_unix_nano: 0,
                        observed_time_unix_nano: 0,
                        severity_number: 9,
                        severity_text: String::new(),
                        body: Some(av_string("msg")),
                        attributes: vec![kv("ai.tool", av_string(&"x".repeat(65)))],
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: vec![],
                        span_id: vec![],
                        event_name: String::new(),
                    },
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let entries = build_entries(&req, peer);
    assert_eq!(entries[0].ai_tool, None);
    assert_eq!(entries[1].ai_tool, None);
}

// ---- key_strindex (syslog-mcp-774ia) ----

#[test]
fn attr_key_resolves_normal_keys() {
    let entry = kv("host.name", av_string("x"));
    assert_eq!(attr_key(&entry), Some("host.name"));
}

#[test]
fn attr_key_passes_through_genuinely_empty_key() {
    let entry = KeyValue {
        key: String::new(),
        value: Some(av_string("x")),
        key_strindex: 0,
    };
    assert_eq!(attr_key(&entry), Some(""));
}

#[test]
fn attr_key_skips_unresolved_string_table_indexed_key() {
    let entry = KeyValue {
        key: String::new(),
        value: Some(av_string("x")),
        key_strindex: 5,
    };
    assert_eq!(attr_key(&entry), None);
}

// Per the OTLP wire format, key_strindex takes precedence over key whenever
// it's set (nonzero), even if key also holds a literal value -- a
// non-conformant producer sending both must not have the literal key
// silently win over the (spec-authoritative) index.
#[test]
fn attr_key_skips_when_key_strindex_set_even_if_key_is_also_populated() {
    let entry = KeyValue {
        key: "host.name".to_string(),
        value: Some(av_string("x")),
        key_strindex: 5,
    };
    assert_eq!(attr_key(&entry), None);
}

#[test]
fn build_entries_skips_string_table_indexed_keys_instead_of_colliding() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    kv("host.name", av_string("nashost")),
                    KeyValue {
                        key: String::new(),
                        value: Some(av_string("unresolvable-1")),
                        key_strindex: 1,
                    },
                    KeyValue {
                        key: String::new(),
                        value: Some(av_string("unresolvable-2")),
                        key_strindex: 2,
                    },
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: Some(av_string("msg")),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let entries = build_entries(&req, peer);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hostname, "nashost");
    let metadata: serde_json::Value =
        serde_json::from_str(entries[0].metadata_json.as_deref().unwrap()).unwrap();
    let resource_attrs = &metadata["resource_attributes"];
    assert_eq!(resource_attrs["host.name"], "nashost");
    // Both string-table-indexed attributes are skipped rather than
    // colliding onto (and clobbering each other via) an "" key.
    assert!(resource_attrs.get("").is_none());
}

#[test]
fn build_entries_ignores_oversized_ai_project_and_session_id() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    kv("host.name", av_string("nashost")),
                    kv("project.path", av_string(&"p".repeat(513))),
                    kv("session.id", av_string(&"s".repeat(129))),
                ],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: Some(av_string("msg")),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let entries = build_entries(&req, peer);
    assert_eq!(entries[0].ai_project, None);
    assert_eq!(entries[0].ai_session_id, None);
}
