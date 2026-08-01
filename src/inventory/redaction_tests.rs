use super::*;
use crate::inventory::limits::MAX_ARRAY_ENTRIES;
use serde_json::json;

#[test]
fn redacts_url_query_headers_and_jwt_like_values() {
    let auth_header = ["Authoriza", "tion"].concat();
    let bearer = ["Bea", "rer"].concat();
    let jwt_like = ["abcdefgh", "ijklmnop", "qrstuvwxyz123456"].join(".");
    let token_key = ["tok", "en"].concat();
    let password_key = ["pass", "word"].concat();
    let input = format!(
        "{auth_header}: {bearer} {jwt_like} {token_key}=remove-me url=https://x/?{token_key}=url-remove-me {password_key}=pw-remove-me"
    );
    let out = redact_text(&input);
    assert!(!out.contains("remove-me"));
    assert!(!out.contains("url-remove-me"));
    assert!(!out.contains("pw-remove-me"));
    assert!(!out.contains("qrstuvwxyz123456"));
    assert!(out.contains("[REDACTED]"));
}

#[test]
fn redacts_generic_key_query_parameters_without_touching_safe_queries() {
    let secret = "remove-this-query-key";
    let input = format!("https://dns.example/dns-query?profile=home&key={secret}&mode=strict");

    let out = redact_text(&input);

    assert!(!out.contains(secret));
    assert!(out.contains("profile=home"));
    assert!(out.contains("key=[REDACTED]"));
    assert!(out.contains("mode=strict"));
}

#[test]
fn redaction_preserves_domain_like_three_segment_strings() {
    let value = "com.docker.compose.project.name";
    assert_eq!(redact_text(value), value);
}

#[test]
fn redacts_cli_userinfo_and_private_key_blocks() {
    let private_key_label = ["PRI", "VATE ", "KEY"].concat();
    let pem_fence = "-".repeat(5);
    let private_key_block = format!(
        "{pem_fence}BEGIN {private_key_label}{pem_fence}\nfixture\n{pem_fence}END {private_key_label}{pem_fence}"
    );
    let curl_user_flag = ["-", "u"].concat();
    let fixture_user = ["fixture", "user"].join("-");
    let fixture_pass = ["fixture", "pass"].join("-");
    let fixture_host = ["fixture", ".test"].concat();
    let input = format!(
        "curl {curl_user_flag} {fixture_user}:{fixture_pass} https://{fixture_host} https://{fixture_user}:{fixture_pass}@{fixture_host} {private_key_block}"
    );

    let out = redact_text(&input);

    assert!(!out.contains(&fixture_pass));
    assert!(!out.contains("fixture\n"));
    assert!(out.contains(&format!("curl {curl_user_flag} [REDACTED]")));
    assert!(out.contains("https://[REDACTED]fixture.test"));
}

#[test]
fn redacts_recursive_json_and_name_value_pairs() {
    let input = json!({
        "ssid": "main",
        "psk": "wifi-remove-me",
        "headers": {"x-api-key": "abc123"},
        "pairs": [{"Name": "password", "Value": "hidden"}]
    });
    let out = redact_json(&input);
    let text = serde_json::to_string(&out).unwrap();
    assert!(!text.contains("wifi-remove-me"));
    assert!(!text.contains("abc123"));
    assert!(!text.contains("hidden"));
}

#[test]
fn redacts_quoted_config_assignments() {
    let token_key = ["tok", "en"].concat();
    let password_key = ["pass", "word"].concat();
    let quoted_a = ["quoted", "remove", "me"].join("-");
    let quoted_b = ["single", "remove", "me"].join("-");
    let input = format!(r#"{token_key}: "{quoted_a}" {password_key}='{quoted_b}'"#);

    let out = redact_text(&input);

    assert!(!out.contains(&quoted_a));
    assert!(!out.contains(&quoted_b));
    assert!(out.contains("[REDACTED]"));
}

#[test]
fn redacted_json_marks_truncated_arrays() {
    let input = json!((0..=MAX_ARRAY_ENTRIES).collect::<Vec<_>>());
    let out = redact_json(&input);
    let items = out.as_array().unwrap();
    assert_eq!(items.len(), MAX_ARRAY_ENTRIES + 1);
    assert_eq!(items.last().unwrap(), "[TRUNCATED_ARRAY]");
}

#[test]
fn redaction_preserves_non_secret_uuids() {
    let id = "550e8400-e29b-41d4-a716-446655440000";
    assert_eq!(redact_text(id), id);
}

#[test]
fn redacted_artifact_reports_status_and_truncation() {
    let token_key = ["tok", "en"].concat();
    let input = format!("{} {token_key}=remove-me", "safe ".repeat(64));
    let artifact = RedactedArtifact::from_text(&input, 24);
    assert!(artifact.truncated());
    assert_eq!(artifact.status(), RedactionStatus::Redacted);
    assert!(!artifact.body().contains("remove-me"));
}
