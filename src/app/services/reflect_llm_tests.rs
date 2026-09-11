use super::*;

fn internal(error: LlmRunnerError) -> ServiceError {
    ServiceError::Internal(anyhow::anyhow!(error))
}

#[test]
fn globally_disabled_llm_is_unavailable() {
    assert!(matches!(
        classify_llm_failure(&internal(LlmRunnerError::Disabled)),
        ReflectLlmFailure::Unavailable(_)
    ));
}

#[test]
fn action_disabled_and_open_circuit_block_one_kind() {
    assert!(matches!(
        classify_llm_failure(&internal(LlmRunnerError::ActionDisabled(
            "skill_assess".into()
        ))),
        ReflectLlmFailure::KindBlocked(_)
    ));
    let ReflectLlmFailure::KindBlocked(reason) =
        classify_llm_failure(&internal(LlmRunnerError::CircuitOpen {
            action: "mcp_assess".into(),
            retry_after: "2026-09-11T00:05:00Z".into(),
        }))
    else {
        panic!("expected KindBlocked");
    };
    assert!(reason.contains("circuit open"));
}

#[test]
fn timeouts_rate_limits_and_backend_errors_fail_one_incident() {
    for error in [
        LlmRunnerError::Timeout("id".into(), 120),
        LlmRunnerError::RateLimited {
            action: "a".into(),
            detail: "d".into(),
        },
        LlmRunnerError::Internal(anyhow::anyhow!("spawn failed")),
    ] {
        assert!(matches!(
            classify_llm_failure(&internal(error)),
            ReflectLlmFailure::Failed(_)
        ));
    }
}

#[test]
fn non_llm_errors_propagate() {
    assert_eq!(
        classify_llm_failure(&ServiceError::InvalidInput("bad".into())),
        ReflectLlmFailure::NotLlm
    );
}

#[test]
fn program_lookup_uses_the_exact_string() {
    assert!(program_on_path("sh"));
    assert!(program_on_path("/bin/sh"));
    assert!(!program_on_path("cortex-reflect-definitely-missing-binary"));
    assert!(!program_on_path(""));
    // A path containing a space must not be split into "/tmp/My".
    let dir = tempfile::tempdir().unwrap();
    let spaced = dir.path().join("My Codex");
    std::fs::create_dir_all(&spaced).unwrap();
    let program = spaced.join("codex");
    std::fs::write(&program, "").unwrap();
    assert!(program_on_path(program.to_str().unwrap()));
}
