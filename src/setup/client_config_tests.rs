use super::*;
#[test]
fn malformed_codex_diagnostics_never_echo_credential_source() {
    let raw = "[mcp_servers.cortex]\nhttp_headers = { Authorization = \"Bearer CREDENTIAL_CANARY\", BROKEN }\n";
    let error = render("codex", raw, "https://example.test/mcp", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("TOML syntax error at line 2"));
    assert!(!error.contains("CREDENTIAL_CANARY"));
    assert!(!error.contains("Authorization"));
}
#[test]
fn preserves_comments_other_servers_and_advanced_options() {
    let raw = "# keep me\nmodel = 'x'\n[mcp_servers.other]\nurl = 'https://other'\n[mcp_servers.cortex]\nstartup_timeout_sec = 30\ncommand = 'old'\n";
    let out = render("codex", raw, "https://cortex.test/mcp", Some("credential")).unwrap();
    assert!(
        out.contains("# keep me")
            && out.contains("startup_timeout_sec = 30")
            && out.contains("mcp_servers.other")
    );
    assert!(!out.contains("command ="));
    let oauth = render("codex", &out, "https://cortex.test/mcp", None).unwrap();
    assert!(!oauth.contains("credential"));
}
#[test]
fn json_clients_preserve_other_settings() {
    for client in ["claude", "gemini"] {
        let out = render(
            client,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"safe"},"cortex":{"timeout":30}}}"#,
            "https://cortex.test/mcp",
            None,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "safe");
        assert_eq!(v["mcpServers"]["cortex"]["timeout"], 30);
    }
}
#[test]
fn malformed_existing_config_never_gets_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(".claude.json"), "broken").unwrap();
    assert!(
        configure_clients(
            temp.path(),
            &["claude".into()],
            "https://cortex.test",
            None,
            false
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".claude.json")).unwrap(),
        "broken"
    );
}
#[test]
fn dry_run_has_no_filesystem_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    configure_clients(
        temp.path(),
        &["claude".into(), "gemini".into()],
        "https://cortex.test",
        None,
        true,
    )
    .unwrap();
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn configuring_claude_preserves_home_permissions() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    configure_clients(
        dir.path(),
        &["claude".into()],
        "https://example.test",
        Some("secret"),
        false,
    )
    .unwrap();
    assert_eq!(std::fs::metadata(dir.path()).unwrap().mode() & 0o777, 0o755);
    assert_eq!(
        std::fs::metadata(dir.path().join(".claude.json"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn codex_preserves_custom_headers_and_replaces_authorization_case_insensitively() {
    for raw in [
        "[mcp_servers.cortex]\ncommand = 'old'\nargs = ['x']\nenv = { OLD = 'x' }\nbearer_token_env_var = 'OLD_TOKEN'\nhttp_headers = { authorization = 'old', X-Tenant = 'tenant' }\nenv_http_headers = { AUTHORIZATION = 'OLD_TOKEN', X-Trace = 'TRACE_ENV' }\n",
        "[mcp_servers.cortex]\n[mcp_servers.cortex.http_headers]\nauthorization = 'old'\nX-Tenant = 'tenant'\n[mcp_servers.cortex.env_http_headers]\nAUTHORIZATION = 'OLD_TOKEN'\nX-Trace = 'TRACE_ENV'\n",
    ] {
        let out = render("codex", raw, "https://example.test/mcp", Some("new")).unwrap();
        let doc = out.parse::<toml_edit::DocumentMut>().unwrap();
        let server = &doc["mcp_servers"]["cortex"];
        assert_eq!(server["http_headers"]["X-Tenant"].as_str(), Some("tenant"));
        assert_eq!(
            server["env_http_headers"]["X-Trace"].as_str(),
            Some("TRACE_ENV")
        );
        assert_eq!(
            server["http_headers"]["Authorization"].as_str(),
            Some("Bearer new")
        );
        assert!(
            !out.contains("OLD_TOKEN") && !out.contains("command =") && !out.contains("args =")
        );
        let oauth = render("codex", &out, "https://example.test/mcp", None).unwrap();
        assert!(oauth.contains("X-Tenant") && oauth.contains("X-Trace"));
        assert!(!oauth.to_ascii_lowercase().contains("authorization"));
    }
}

#[test]
fn json_preserves_custom_headers_when_replacing_or_removing_authentication() {
    for client in ["claude", "gemini"] {
        let raw = r#"{"mcpServers":{"cortex":{"command":"old","args":[],"env":{},"headers":{"authorization":"old","AUTHORIZATION":"other","X-Tenant":"tenant"}}}}"#;
        let out = render(client, raw, "https://example.test/mcp", Some("new")).unwrap();
        let doc: Value = serde_json::from_str(&out).unwrap();
        let server = &doc["mcpServers"]["cortex"];
        assert_eq!(
            server["headers"],
            json!({"Authorization":"Bearer new","X-Tenant":"tenant"})
        );
        assert!(
            server.get("command").is_none()
                && server.get("args").is_none()
                && server.get("env").is_none()
        );
        let oauth: Value =
            serde_json::from_str(&render(client, &out, "https://example.test/mcp", None).unwrap())
                .unwrap();
        assert_eq!(
            oauth["mcpServers"]["cortex"]["headers"],
            json!({"X-Tenant":"tenant"})
        );
    }
}

#[test]
fn malformed_headers_fail_instead_of_discarding_existing_configuration() {
    assert!(
        render(
            "codex",
            "[mcp_servers.cortex]\nhttp_headers = 'invalid'",
            "https://example.test/mcp",
            None
        )
        .is_err()
    );
    for client in ["claude", "gemini"] {
        assert!(
            render(
                client,
                r#"{"mcpServers":{"cortex":{"headers":"invalid"}}}"#,
                "https://example.test/mcp",
                None
            )
            .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn final_config_rejects_symlinks_and_foreign_ownership() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    std::fs::write(&target, "original").unwrap();
    let path = temp.path().join(".claude.json");
    symlink(&target, &path).unwrap();
    assert!(read_config(&path).is_err());
    assert!(atomic_private_write(&path, b"replacement", 0o600).is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    let uid = unsafe { libc::geteuid() };
    assert!(validate_owner(uid).is_ok());
    assert_eq!(
        validate_owner(uid.wrapping_add(1)).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn json_client_enrollment_remains_byte_stable_across_repeats() {
    for client in ["claude", "gemini"] {
        let raw = r#"{"mcpServers":{"cortex":{"command":"old", "headers":{"X-Custom":"keep"}}}}"#;
        let first = render(client, raw, "https://cortex.test/mcp", Some("token")).unwrap();
        let second = render(client, &first, "https://cortex.test/mcp", Some("token")).unwrap();
        assert_eq!(first, second);
        assert!(second.contains("X-Custom"));
    }
}
