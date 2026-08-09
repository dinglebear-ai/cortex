use super::models::*;

#[test]
fn add_request_builds_enabled_source_with_defaults() {
    let req = FileTailAddRequest {
        id: "swag-access".into(),
        path: "/mnt/appdata/swag/log/nginx/access.log".into(),
        tag: "swag-access".into(),
        host: Some("edgehost".into()),
        facility: None,
        severity: None,
        start_at_end: None,
    };

    let source = FileTailSource::from_add(req, "2026-06-11T20:00:00Z").unwrap();

    assert_eq!(source.id, "swag-access");
    assert_eq!(source.path, "/mnt/appdata/swag/log/nginx/access.log");
    assert_eq!(source.tag, "swag-access");
    assert_eq!(source.hostname.as_deref(), Some("edgehost"));
    assert_eq!(source.facility.as_deref(), Some("local7"));
    assert_eq!(source.severity, "info");
    assert!(source.start_at_end);
    assert!(source.enabled);
    assert_eq!(source.created_at, "2026-06-11T20:00:00Z");
    assert_eq!(source.updated_at, "2026-06-11T20:00:00Z");
}

#[test]
fn add_request_normalizes_and_validates_hostname() {
    let source = FileTailSource::from_add(
        FileTailAddRequest {
            id: "swag-access".into(),
            path: "/mnt/appdata/swag/log/nginx/access.log".into(),
            tag: "swag-access".into(),
            host: Some(" Edgehost.LOCAL ".into()),
            facility: None,
            severity: None,
            start_at_end: None,
        },
        "2026-06-11T20:00:00Z",
    )
    .unwrap();
    assert_eq!(source.hostname.as_deref(), Some("edgehost.local"));

    let err = FileTailSource::from_add(
        FileTailAddRequest {
            id: "bad-host".into(),
            path: "/mnt/appdata/swag/log/nginx/access.log".into(),
            tag: "bad-host".into(),
            host: Some("bad host/name".into()),
            facility: None,
            severity: None,
            start_at_end: None,
        },
        "2026-06-11T20:00:00Z",
    )
    .unwrap_err();
    assert!(err.contains("hostname must be URI-safe"));
}

#[test]
fn file_tail_request_rejects_missing_fields_for_add() {
    let req = FileTailRequest {
        op: FileTailOp::Add,
        id: None,
        path: None,
        tag: None,
        host: None,
        facility: None,
        severity: None,
        start_at_end: None,
    };

    assert_eq!(
        req.into_add().unwrap_err(),
        "file_tails op=add requires path"
    );
}

#[test]
fn file_tail_request_derives_a_human_friendly_add_from_path() {
    let req = FileTailRequest {
        op: FileTailOp::Add,
        id: None,
        path: Some("/mnt/appdata/swag/log/nginx/access.log".into()),
        tag: None,
        host: None,
        facility: None,
        severity: None,
        start_at_end: None,
    };

    let add = req.into_add().unwrap();
    assert_eq!(add.id, "access");
    assert_eq!(add.tag, "access");
    assert_eq!(add.host.as_deref(), Some("file-tail-access"));

    let source = FileTailSource::from_add(add, "2026-07-16T12:00:00Z").unwrap();
    assert_eq!(source.hostname.as_deref(), Some("file-tail-access"));
}

#[test]
fn add_request_derives_stable_source_owner_when_hostname_is_missing() {
    let source = FileTailSource::from_add(
        FileTailAddRequest {
            id: "swag-access".into(),
            path: "/mnt/appdata/swag/log/nginx/access.log".into(),
            tag: "swag-access".into(),
            host: None,
            facility: None,
            severity: None,
            start_at_end: None,
        },
        "2026-06-11T20:00:00Z",
    )
    .unwrap();

    assert_eq!(source.hostname.as_deref(), Some("file-tail-swag-access"));
}

#[test]
fn legacy_source_owner_derivation_is_ascii_safe_and_bounded() {
    let hostname = derived_source_hostname(&format!("🔥{}", "x".repeat(300)));

    assert!(hostname.is_ascii());
    assert!(hostname.len() <= 255);
    assert!(hostname.starts_with("file-tail-"));
}

#[test]
fn file_tail_request_rejects_path_traversal_ids() {
    let req = FileTailRequest {
        op: FileTailOp::Remove,
        id: Some("../swag".into()),
        path: None,
        tag: None,
        host: None,
        facility: None,
        severity: None,
        start_at_end: None,
    };

    assert_eq!(
        req.required_id().unwrap_err(),
        "file_tails id must contain only ASCII letters, digits, dot, underscore, or dash"
    );
}

#[test]
fn file_tail_request_rejects_extra_fields_for_id_ops() {
    let mut req = FileTailRequest::id_op(FileTailOp::Remove, "swag".into());
    req.path = Some("/tmp/access.log".into());

    assert_eq!(
        req.validate_shape().unwrap_err(),
        "file_tails op=remove accepts only id"
    );
}

#[test]
fn file_tail_request_rejects_extra_fields_for_list_ops() {
    let mut req = FileTailRequest::list();
    req.tag = Some("swag".into());

    assert_eq!(
        req.validate_shape().unwrap_err(),
        "file_tails op=list does not accept source fields"
    );
}
