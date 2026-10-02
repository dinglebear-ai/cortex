use super::*;
#[test]
fn schema_covers_deployment_and_collection_settings() {
    let keys = supported_env_keys();
    for key in [
        "CORTEX_ALLOWED_SOURCE_CIDRS",
        "CORTEX_MCP_BIND",
        "CORTEX_MEMORY_LIMIT",
        "CORTEX_BACKUP_DIR",
        "CORTEX_AGENT_FILE_TAILS",
        "CORTEX_TOKEN",
    ] {
        assert!(keys.contains(key), "{key}");
    }
    assert!(!keys.contains("CORTEX_HOME"));
    assert!(validate_setting("CORTEX_MEMORY_LIMIT", "4G").is_ok());
    assert!(validate_setting("CORTEX_UNKNOWN_SETTING", "x").is_err());
    assert!(validate_setting("CORTEX_TOKEN", "x\ny").is_err());
}
#[test]
fn secret_classification_covers_credentials() {
    for key in [
        "CORTEX_TOKEN",
        "CORTEX_API_ADMIN_TOKEN",
        "CORTEX_GOOGLE_CLIENT_SECRET",
        "CORTEX_INTEGRATION_CREDENTIAL_KEY",
        "CORTEX_UNRAID_API_KEY",
    ] {
        assert!(secret_key(key));
    }
    assert!(!secret_key("CORTEX_PORT"));
}
#[test]
fn effective_config_redacts_generated_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let view = effective_environment(dir.path()).unwrap();
    assert!(
        view.iter()
            .any(|s| s.key == "CORTEX_API_TOKEN" && s.value == "[REDACTED]")
    );
    assert!(!dir.path().join(".env").exists());
}

#[test]
fn endpoint_credentials_are_redacted_even_without_a_secret_key() {
    for value in [
        "http://user:password@docker:2375",
        "https://example.test?api_key=private",
        "http://plain:2375,http://user:password@other:2375",
    ] {
        assert_eq!(
            redacted_value("CORTEX_AGENT_DOCKER_URL", value),
            "[REDACTED]"
        );
    }
    assert_eq!(
        redacted_value("CORTEX_AGENT_DOCKER_URL", "http://docker:2375"),
        "http://docker:2375"
    );
    let capability = super::super::AgentCapability {
        id: "docker".into(),
        label: "Docker".into(),
        status: super::super::CapabilityStatus::Enabled,
        available: true,
        detail: "configured".into(),
        env_key: "CORTEX_AGENT_DOCKER".into(),
        detected_value: Some("http://user:password@docker:2375".into()),
    };
    let encoded = serde_json::to_string(&capability).unwrap();
    assert!(encoded.contains("[REDACTED]"));
    assert!(!encoded.contains("password"));
}
