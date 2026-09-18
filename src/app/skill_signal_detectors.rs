//! Deterministic, phrase-boundary keyword detectors for skill-incident anchor
//! signals. Pure functions over log message text — no DB, no LLM. Mirrors the
//! word/phrase-boundary matching style of `first_abuse_term`/`is_abuse_boundary`
//! in `src/db/queries.rs`, but operates on the app layer since these detectors
//! are shared by both the DB-layer grouping query and the evidence bundle
//! nearby-log classification.

pub const SIGNAL_USER_CORRECTION_AFTER_SKILL: &str = "user_correction_after_skill";
pub const SIGNAL_TOOL_FAILURE_AFTER_SKILL: &str = "tool_failure_after_skill";
pub const SIGNAL_SCOPE_OR_SOURCE_CONFUSION: &str = "scope_or_source_confusion";
pub const SIGNAL_IGNORED_SKILL_OR_POLICY_INSTRUCTION: &str = "ignored_skill_or_policy_instruction";
pub const SIGNAL_OVERLONG_LOOP_AFTER_SKILL: &str = "overlong_loop_after_skill";

/// All five locked anchor signal categories, in a stable order used for
/// `signals_present` sorting and CLI `--signals` validation.
pub const ALL_SIGNALS: &[&str] = &[
    SIGNAL_USER_CORRECTION_AFTER_SKILL,
    SIGNAL_TOOL_FAILURE_AFTER_SKILL,
    SIGNAL_SCOPE_OR_SOURCE_CONFUSION,
    SIGNAL_IGNORED_SKILL_OR_POLICY_INSTRUCTION,
    SIGNAL_OVERLONG_LOOP_AFTER_SKILL,
];

/// Phrases indicating the user is correcting or pushing back on the assistant
/// immediately after a skill loaded. Deliberately phrase-level (not single
/// words like "no" or "wrong" in isolation) to avoid false positives on
/// unrelated negatives ("no new errors were found").
const USER_CORRECTION_PHRASES: &[&str] = &[
    "that's not what i asked",
    "that is not what i asked",
    "you said you would",
    "but you didn't",
    "but you did not",
    "is wrong",
    "is just wrong",
    "no, that's",
    "no, that is",
    "we wasted",
    "all you had to say",
    "stop, you",
    "you didn't need to",
    "you did not need to",
];

/// Phrases indicating a tool/command failure surfaced in nearby transcript or
/// tool-output text.
const TOOL_FAILURE_PHRASES: &[&str] = &[
    "exit code",
    "permission denied",
    "not found",
    "timed out",
    "failed to",
    "database is locked",
    "rate limit",
];

/// Case-insensitive substring match on whole phrases (already multi-word, so
/// word-boundary checks are unnecessary for most entries — a phrase like
/// "you said" cannot ride inside an unrelated longer word the way a bare
/// single-word term like "hell" could).
fn contains_any_phrase(haystack_lower: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| haystack_lower.contains(p))
}

/// Return true only for transcript rows explicitly persisted as user-authored.
/// Unknown/legacy speaker metadata is intentionally not guessed; extractor
/// revision replay repairs historical rows before incident scoring.
pub fn transcript_event_is_user(metadata_json: Option<&str>) -> bool {
    metadata_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| {
            value
                .get("event_kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some("user")
}

pub fn detect_user_correction(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, USER_CORRECTION_PHRASES)
}

pub fn detect_tool_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, TOOL_FAILURE_PHRASES)
}

/// Conservative phrases indicating the assistant is operating on the wrong
/// repo, stale data, wrong source of truth, or confusing an in-memory/cached
/// view with the live system. Starts from the same conservative style as
/// `UNCLEAR_INSTRUCTION_OR_SCOPE_DRIFT` in `src/app/incident_findings.rs`
/// plus skill-specific additions for source-of-truth confusion.
const SCOPE_OR_SOURCE_CONFUSION_PHRASES: &[&str] = &[
    "wrong repo",
    "wrong file",
    "stale data",
    "wrong source",
    "source of truth",
    "memory-vs-live",
    "memory vs live",
    "not the live",
    "going in circles",
];

/// Explicit-violation phrases for a fixed set of known instruction
/// categories: no verification after implementation, no issue/bead when
/// required, wrong transport/source, raw web when Axon/Labby required,
/// stopped at plan when asked to implement. Deliberately requires explicit
/// phrase evidence — no broad single-word matches.
const IGNORED_INSTRUCTION_PHRASES: &[&str] = &[
    "without any verification",
    "without verification",
    "claimed success without",
    "should have created a bead",
    "should have created an issue",
    "wrong transport",
    "wrong source for this call",
    "raw web instead of using axon",
    "instead of using axon",
    "stopped at the plan",
    "stopped at plan",
];

pub fn detect_scope_or_source_confusion(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, SCOPE_OR_SOURCE_CONFUSION_PHRASES)
}

pub fn detect_ignored_instruction(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_any_phrase(&lower, IGNORED_INSTRUCTION_PHRASES)
}

/// Minimum tool-call volume (rows between the skill event and resolution)
/// that counts as "many" for the overlong-loop signal.
const OVERLONG_LOOP_TOOL_CALL_THRESHOLD: usize = 15;

/// `overlong_loop_after_skill` requires BOTH a high tool-call volume after
/// the skill event AND a co-occurring negative signal (user correction or
/// frustration). Long-but-successful work alone must never trigger this —
/// callers pass `has_correction_or_frustration_signal` computed from the
/// other four detectors / the abuse-term matcher over the same window.
pub fn detect_overlong_loop(
    _skill_event_count: usize,
    tool_call_count: usize,
    has_correction_or_frustration_signal: bool,
) -> bool {
    tool_call_count >= OVERLONG_LOOP_TOOL_CALL_THRESHOLD && has_correction_or_frustration_signal
}

#[cfg(test)]
#[path = "skill_signal_detectors_tests.rs"]
mod tests;
