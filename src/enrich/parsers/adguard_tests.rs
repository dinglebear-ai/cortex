use crate::enrich::{Parser, ParserInput, SourceKind};

fn input_from(fixture: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/parsers/adguard/{fixture}"))
        .unwrap()
        .trim()
        .to_string()
}

fn parse(
    message: &str,
    source_kind: SourceKind,
) -> Result<crate::enrich::ParserOutput, crate::enrich::ParserError> {
    super::AdguardParser.parse(ParserInput {
        app_name: Some("adguard-query"),
        container_name: None,
        message,
        raw: message,
        source_kind,
        severity: "info",
    })
}

#[test]
fn block_marks_dns_blocked_true() {
    let out = parse(&input_from("block.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.dns_blocked, Some(true));
    assert_eq!(out.event_action.as_deref(), Some("dns_query"));
    assert_eq!(out.metadata["query"], serde_json::json!("doubleclick.net"));
    assert_eq!(out.metadata["qtype"], serde_json::json!("A"));
    assert_eq!(out.metadata["client"], serde_json::json!("192.168.10.55"));
    assert_eq!(
        out.metadata["reason"],
        serde_json::json!("FilteredBlackList")
    );
    assert_eq!(
        out.metadata["rule"],
        serde_json::json!("||doubleclick.net^")
    );
}

#[test]
fn allow_marks_dns_blocked_false() {
    let out = parse(&input_from("allow.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.dns_blocked, Some(false));
}

#[test]
fn rewrite_marks_dns_blocked_null() {
    let out = parse(&input_from("rewrite.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.dns_blocked, None);
    assert_eq!(out.metadata["reason"], serde_json::json!("Rewrite"));
}

#[test]
fn cached_hit() {
    let out = parse(&input_from("cached_hit.json"), SourceKind::DockerStream).unwrap();
    assert_eq!(out.metadata["cached"], serde_json::json!(true));
}

#[test]
fn legacy_camelcase_falls_back() {
    let out = parse(
        &input_from("legacy_camelcase.json"),
        SourceKind::DockerStream,
    )
    .unwrap();
    assert_eq!(out.metadata["query"], serde_json::json!("example.com"));
    assert_eq!(out.dns_blocked, Some(false));
}

#[test]
fn api_poller_path_yields_identical_output() {
    let from_docker = parse(&input_from("block.json"), SourceKind::DockerStream).unwrap();
    let from_api = parse(
        &input_from("api_poller_normalised.json"),
        SourceKind::AdguardApi,
    )
    .unwrap();
    assert_eq!(from_docker.dns_blocked, from_api.dns_blocked);
    assert_eq!(from_docker.metadata, from_api.metadata);
}

#[test]
fn truncated_invalid_returns_json_error() {
    let err = parse(
        &input_from("truncated_invalid.txt"),
        SourceKind::DockerStream,
    )
    .unwrap_err();
    assert!(matches!(err, crate::enrich::ParserError::Json(_)));
}

#[test]
fn file_querylog_ip_field_is_used_as_client() {
    // AdGuard Home ≥0.107 file query log records the client as `IP` (not
    // `Client`), with a `Question` map for the domain. The parser must still
    // surface the client + query for graph device/domain extraction.
    let line = r#"{"T":"2026-06-18T16:01:45Z","QH":"www.microsoft.com","QT":"A","IP":"198.51.100.1","Result":{},"Cached":false}"#;
    let out = parse(line, SourceKind::FileTail).unwrap();
    assert_eq!(
        out.metadata["query"],
        serde_json::json!("www.microsoft.com")
    );
    assert_eq!(out.metadata["client"], serde_json::json!("198.51.100.1"));
}

#[test]
fn cid_field_is_used_as_client_when_no_ip() {
    // `CID` (persistent client id) is the last fallback in the client-field
    // priority; a record carrying only `CID` must still resolve a client.
    let line = r#"{"QH":"example.org","QT":"A","CID":"living-room-tv","Result":{},"Cached":false}"#;
    let out = parse(line, SourceKind::FileTail).unwrap();
    assert_eq!(out.metadata["client"], serde_json::json!("living-room-tv"));
}

#[test]
fn client_field_precedence_prefers_ip_over_cid() {
    // Priority order is Client, client, IP, CID — so a record with both `IP`
    // and `CID` must surface the network address, not the persistent id.
    let line = r#"{"QH":"example.org","QT":"A","IP":"10.0.0.5","CID":"living-room-tv","Result":{},"Cached":false}"#;
    let out = parse(line, SourceKind::FileTail).unwrap();
    assert_eq!(out.metadata["client"], serde_json::json!("10.0.0.5"));
}
