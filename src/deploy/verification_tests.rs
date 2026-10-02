use super::*;

#[test]
fn authenticated_verification_rejects_rpc_errors_and_wrong_ids() {
    let valid = r#"{"id":1,"result":{"serverInfo":{}}}__CORTEX_REPLY__{"id":2,"result":{"content":[]}}__CORTEX_REPLY__{}__CORTEX_REPLY__{}"#;
    assert!(validate_replies(valid, false).is_ok());
    assert!(validate_replies(&valid.replace("\"id\":2", "\"id\":9"), false).is_err());
    assert!(
        validate_replies(
            &valid.replace("\"content\":[]", "\"content\":[],\"isError\":true"),
            false
        )
        .is_err()
    );
    assert!(validate_replies("{}", false).is_err());
}

#[test]
fn oauth_only_does_not_require_or_enable_static_mcp_tokens() {
    assert!(
        validate_replies(
            "null__CORTEX_REPLY__null__CORTEX_REPLY__{}__CORTEX_REPLY__{}",
            true
        )
        .is_ok()
    );
    assert!(
        validate_replies(
            "null__CORTEX_REPLY__null__CORTEX_REPLY__{}__CORTEX_REPLY__{}",
            false
        )
        .is_err()
    );
}

#[test]
fn parses_json_rpc_sse_responses() {
    assert_eq!(
        parse_reply("event: message\ndata: {\"id\":1}\n\n").unwrap()["id"],
        1
    );
}
