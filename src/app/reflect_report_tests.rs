use super::*;
use crate::app::models::{
    ReflectAssessed, ReflectIncident, ReflectIndexSummary, ReflectKind, ReflectKindSummary,
    ReflectMode, ReflectReport,
};

fn incident(kind: ReflectKind, id: &str, target: &str) -> ReflectIncident {
    ReflectIncident {
        kind,
        incident_id: id.to_string(),
        target: target.to_string(),
        target_key: target.to_string(),
        target_detail: None,
        tool: "claude".to_string(),
        project: "/p".to_string(),
        session_id: "sess-1".to_string(),
        last_seen: "2026-09-10T12:00:00Z".to_string(),
        priority_score: 42.0,
        priority_label: "high".to_string(),
        signals_present: vec!["user_correction_after_skill".to_string()],
    }
}

fn summary_row(
    kind: ReflectKind,
    listed: usize,
    total: usize,
    truncated: bool,
) -> ReflectKindSummary {
    ReflectKindSummary {
        kind,
        listed,
        total,
        truncated,
        critical: 0,
        high: listed,
        medium: 0,
        low: 0,
    }
}

fn report(assessed: Vec<ReflectAssessed>, unassessed: Vec<ReflectIncident>) -> ReflectReport {
    ReflectReport {
        since: Some("2026-09-04T00:00:00Z".to_string()),
        until: Some("2026-09-11T00:00:00Z".to_string()),
        project: None,
        tool: None,
        kinds: ReflectKind::ALL.to_vec(),
        db_path: "/home/u/.cortex/reflect.db".to_string(),
        mode: ReflectMode::ReportOnly,
        llm_fallback_reason: None,
        index: Some(ReflectIndexSummary {
            discovered_files: 3,
            ingested: 10,
            skipped_dupes: 0,
            parse_errors: 1,
        }),
        summary: vec![summary_row(ReflectKind::Skill, 1, 1, false)],
        assessed,
        unassessed,
    }
}

fn assessed(target: &str, assessment: Option<&str>, failure: Option<&str>) -> ReflectAssessed {
    ReflectAssessed {
        incident: incident(ReflectKind::Skill, "abc", target),
        assessment: assessment.map(str::to_string),
        findings: serde_json::json!({"likely_failure_modes": ["x"]}),
        failure: failure.map(str::to_string),
    }
}

#[test]
fn report_only_shows_summary_findings_and_other_incidents() {
    let md = render_reflect_markdown(&report(
        vec![assessed("lavra:lavra-plan", None, None)],
        vec![incident(ReflectKind::Mcp, "def", "labby/search")],
    ));
    assert!(md.starts_with("# Cortex reflect report\n"));
    assert!(md.contains("Mode: report only"));
    assert!(md.contains(
        "Index: 3 files discovered, 10 new records, 0 duplicates skipped, 1 parse errors"
    ));
    assert!(md.contains("| skill | 1 | 1 | 0 | 1 | 0 | 0 |"));
    assert!(md.contains("Scores are heuristic"));
    assert!(md.contains("## 1. skill `lavra:lavra-plan` (high, score 42.0)"));
    assert!(md.contains("```json\n"));
    assert!(md.contains("## Other incidents"));
    assert!(md.contains(
        "`cortex assess mcp --incident-id def --since 2026-09-04T00:00:00Z --until 2026-09-11T00:00:00Z`"
    ));
}

#[test]
fn truncated_kinds_get_a_warning() {
    let mut rep = report(vec![], vec![incident(ReflectKind::Skill, "a", "s")]);
    rep.summary = vec![summary_row(ReflectKind::Skill, 100, 250, true)];
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("> skill: showing the top 100 of 250 incidents"));
    assert!(md.contains("narrow --since"));
}

#[test]
fn llm_headings_are_demoted_outside_code_fences() {
    let mut rep = report(
        vec![assessed(
            "s",
            Some("## Incident Summary\nText\n```sh\n# keep me\n```\n### Detail\n"),
            None,
        )],
        vec![],
    );
    rep.mode = ReflectMode::ReportAndLlm;
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("Mode: report + LLM"));
    assert!(md.contains("\n#### Incident Summary\n"));
    assert!(md.contains("\n# keep me\n"));
    assert!(md.contains("\n##### Detail\n"));
    assert!(!md.contains("```json"));
}

#[test]
fn failures_and_fallbacks_are_visible() {
    let mut rep = report(
        vec![assessed(
            "s",
            None,
            Some("LLM invocation 'x' timed out after 120s"),
        )],
        vec![],
    );
    rep.llm_fallback_reason = Some("LLM backend program 'codex' was not found".to_string());
    rep.index = None;
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("> LLM skipped: LLM backend program 'codex' was not found"));
    assert!(md.contains("- Assessment failed: LLM invocation 'x' timed out after 120s"));
    assert!(md.contains("Index: skipped (--no-index)"));
}

#[test]
fn null_findings_are_not_rendered_as_json() {
    let mut entry = assessed("s", None, Some("incident changed during the run"));
    entry.findings = serde_json::Value::Null;
    let md = render_reflect_markdown(&report(vec![entry], vec![]));
    assert!(!md.contains("```json"));
}

#[test]
fn empty_report_says_so() {
    let mut rep = report(vec![], vec![]);
    rep.summary = vec![];
    let md = render_reflect_markdown(&rep);
    assert!(md.contains("No skill, MCP, or hook incidents found in this window."));
    assert!(!md.contains("## Other incidents"));
}

#[test]
fn control_characters_and_table_breakers_are_neutralized() {
    let md = render_reflect_markdown(&report(
        vec![assessed("evil\u{1b}[31m`x`", Some("ok\u{1b}[2J\n"), None)],
        vec![incident(ReflectKind::Mcp, "p", "a|b`c")],
    ));
    assert!(!md.contains('\u{1b}'));
    assert!(md.contains("a\\|b'c"));
    assert!(md.contains("`evil [31m'x'`"));
}
