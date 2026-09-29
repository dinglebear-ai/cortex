use super::*;

#[test]
fn trace_export_target_is_loopback_without_embedded_secrets() {
    assert!(validate_trace_endpoint("http://127.0.0.1:3100/v1/traces").is_ok());
    assert!(validate_trace_endpoint("http://localhost:3100/v1/traces").is_ok());
    for endpoint in [
        "https://example.com/v1/traces",
        "http://10.0.0.1:3100/v1/traces",
        "http://user:secret@127.0.0.1:3100/v1/traces",
        "http://127.0.0.1:3100/v1/traces?token=secret",
        "http://127.0.0.1:3100/v1/logs",
    ] {
        assert!(validate_trace_endpoint(endpoint).is_err(), "{endpoint}");
    }
}

#[test]
fn trace_sample_ratio_is_bounded() {
    assert_eq!(trace_sample_ratio(None).unwrap(), 0.1);
    assert_eq!(trace_sample_ratio(Some("1")).unwrap(), 1.0);
    for ratio in ["-0.1", "1.1", "NaN", "infinite", "bad"] {
        assert!(trace_sample_ratio(Some(ratio)).is_err());
    }
}
