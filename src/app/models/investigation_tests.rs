use super::*;

#[test]
fn passive_text_redacts_secret_values_before_truncation() {
    for text in [
        "password=passive-secret-value",
        "Authorization: Bearer passive-secret-value",
        "Bearer passive-secret-value",
        "CORTEX_API_TOKEN=passive-secret-value",
        "token=passive-secret-value",
        "sk-proj-passive-secret-value-0123456789abcdefgh",
    ] {
        let output = safe_passive_text(text, 500);
        assert!(
            !output.contains("passive-secret-value"),
            "secret value survived redaction"
        );
        let short = safe_passive_text(text, 16);
        assert!(
            !short.contains("passive-secret"),
            "truncation exposed a partial secret"
        );
    }
    assert_eq!(safe_passive_text("safe\u{1b} text", 500), "safe text");
}
