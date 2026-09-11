//! Pure helpers that decide how `cortex reflect` reacts when an LLM
//! assessment cannot run.

use crate::app::ServiceError;
use crate::app::llm_runner::LlmRunnerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReflectLlmFailure {
    /// The LLM is globally disabled: no more LLM calls this run.
    Unavailable(String),
    /// This kind's action is disabled or its circuit is open: no more LLM
    /// calls for this kind.
    KindBlocked(String),
    /// Only this assessment failed.
    Failed(String),
    /// Not an LLM failure: propagate the error.
    NotLlm,
}

/// The per-evidence helpers wrap `LlmRunnerError` as
/// `ServiceError::Internal(anyhow!(error))` (see `run_llm_with_delta` in
/// `src/app/services.rs`), so `downcast_ref` recovers it.
pub(crate) fn classify_llm_failure(error: &ServiceError) -> ReflectLlmFailure {
    let ServiceError::Internal(inner) = error else {
        return ReflectLlmFailure::NotLlm;
    };
    match inner.downcast_ref::<LlmRunnerError>() {
        Some(runner @ LlmRunnerError::Disabled) => {
            ReflectLlmFailure::Unavailable(runner.to_string())
        }
        Some(runner @ (LlmRunnerError::ActionDisabled(_) | LlmRunnerError::CircuitOpen { .. })) => {
            ReflectLlmFailure::KindBlocked(runner.to_string())
        }
        Some(runner) => ReflectLlmFailure::Failed(runner.to_string()),
        None => ReflectLlmFailure::Failed(format!("{inner:#}")),
    }
}

/// True when `program` is an existing file path, or a bare name found in a
/// `PATH` directory. The backend spawns this exact string, so it is never
/// split on whitespace.
pub(crate) fn program_on_path(program: &str) -> bool {
    if program.is_empty() {
        return false;
    }
    let candidate = std::path::Path::new(program);
    if candidate.components().count() > 1 {
        return candidate.is_file();
    }
    crate::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
#[path = "reflect_llm_tests.rs"]
mod tests;
