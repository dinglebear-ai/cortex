use super::agent_observatory::{
    AgentEventKind, EvidenceTrustLevel, RepositoryObservationKind, RunStatus, StreamEventName,
};
use super::otlp_metrics::MetricInstrumentKind;
use super::otlp_traces::OtelSpanRow;
use std::str::FromStr;

#[test]
fn observatory_text_enums_round_trip_and_reject_unknown_values() {
    for value in RunStatus::ALL {
        assert_eq!(RunStatus::from_str(value.as_str()).unwrap(), *value);
    }
    for value in AgentEventKind::ALL {
        assert_eq!(AgentEventKind::from_str(value.as_str()).unwrap(), *value);
    }
    for value in EvidenceTrustLevel::ALL {
        assert_eq!(
            EvidenceTrustLevel::from_str(value.as_str()).unwrap(),
            *value
        );
    }
    for value in RepositoryObservationKind::ALL {
        assert_eq!(
            RepositoryObservationKind::from_str(value.as_str()).unwrap(),
            *value
        );
    }
    for value in StreamEventName::ALL {
        assert_eq!(StreamEventName::from_str(value.as_str()).unwrap(), *value);
    }
    for value in MetricInstrumentKind::ALL {
        assert_eq!(
            MetricInstrumentKind::from_str(value.as_str()).unwrap(),
            *value
        );
    }

    assert!(RunStatus::from_str("running").is_err());
    assert!(AgentEventKind::from_str("unknown").is_err());
    assert!(EvidenceTrustLevel::from_str("trusted").is_err());
    assert!(RepositoryObservationKind::from_str("poll").is_err());
    assert!(StreamEventName::from_str("run.deleted").is_err());
    assert!(MetricInstrumentKind::from_str("counter").is_err());
}

#[test]
fn observatory_row_structs_use_string_api_keys_and_internal_integer_ids() {
    let span = OtelSpanRow {
        id: 7,
        trace_id: "0123456789abcdef0123456789abcdef".to_string(),
        span_id: "0123456789abcdef".to_string(),
        parent_span_id: None,
        trace_state: None,
        flags: 0,
        span_name: "fixture".to_string(),
        span_kind: 1,
        start_time_unix_nano: 10,
        end_time_unix_nano: 20,
        duration_nano: 10,
        status_code: 0,
        status_message: None,
        hostname: "fixture-host".to_string(),
        service_name: None,
        service_version: None,
        scope_name: None,
        scope_version: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        run_id: None,
        resource_json: "{}".to_string(),
        attributes_json: "{}".to_string(),
        events_json: "[]".to_string(),
        links_json: "[]".to_string(),
        received_at: "2026-01-01T00:00:00.000Z".to_string(),
        content_scrubbed: true,
    };
    assert_eq!(span.id, 7);
    assert_eq!(span.trace_id.len(), 32);
}
