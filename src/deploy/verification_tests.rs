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

#[test]
fn remote_oauth_policy_uses_runtime_spellings() {
    struct Capture(String);
    impl RemoteRunner for Capture {
        fn run(
            &mut self,
            _: &str,
            script: &str,
            _: Option<&str>,
        ) -> io::Result<super::super::remote_support::RemoteOutput> {
            self.0 = script.into();
            Ok(super::super::remote_support::RemoteOutput {
                status_success: true, exit_code: Some(0), stderr: String::new(),
                stdout: r#"{"id":1,"result":{"serverInfo":{}}}__CORTEX_REPLY__{"id":2,"result":{"content":[]}}__CORTEX_REPLY__{}__CORTEX_REPLY__{}"#.into(),
            })
        }
    }
    for value in ["false", "FALSE", "0", "no", "N", "off"] {
        let env = BTreeMap::from([
            ("CORTEX_AUTH_MODE".into(), "oauth".into()),
            (
                "CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH".into(),
                value.into(),
            ),
        ]);
        let mut runner = Capture(String::new());
        let phases = verify_remote_server(&mut runner, "fixture", "/tmp/fixture", &env).unwrap();
        assert_eq!(phases.len(), 1, "{value}");
        assert_eq!(phases[0].status, SetupStatus::Ok);
        // Execute the actual generated shell normalization independently of Docker.
        let start = runner.0.find("case \"$(printf").unwrap();
        let end = runner.0[start..].find("esac").unwrap() + start + 4;
        let script = format!("{}\nprintf %s \"$disable_static\"", &runner.0[start..end]);
        let output = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH", value)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "false");
    }
}
