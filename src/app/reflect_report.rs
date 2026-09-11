//! Markdown rendering for `cortex reflect`. Pure: no I/O. Transcript-derived
//! text is treated as untrusted: control characters are removed before it
//! reaches a terminal.

use std::fmt::Write as _;

use crate::app::models::{ReflectAssessed, ReflectMode, ReflectReport};

pub fn render_reflect_markdown(report: &ReflectReport) -> String {
    let mut md = String::from("# Cortex reflect report\n\n");
    write_header(&mut md, report);
    write_summary(&mut md, report);
    for (position, assessed) in report.assessed.iter().enumerate() {
        write_assessed(&mut md, position + 1, assessed);
    }
    write_unassessed(&mut md, report);
    md
}

/// Single-line text: every control character becomes a space.
fn inline(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Text inside a code span: single line, no backticks.
fn code_span(text: &str) -> String {
    inline(text).replace('`', "'")
}

/// Text inside a table cell: single line, no pipes or backticks.
fn cell(text: &str) -> String {
    code_span(text).replace('|', "\\|")
}

/// Multi-line text: keep newlines and tabs, drop other control characters.
fn block(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Pushes headings two levels down so they nest under the H2 incident
/// heading, leaving fenced code untouched.
fn demote_headings(text: &str) -> String {
    let mut in_fence = false;
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                line.to_string()
            } else if !in_fence && line.starts_with('#') {
                format!("##{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_header(md: &mut String, report: &ReflectReport) {
    let since = report.since.as_deref().unwrap_or("beginning");
    let until = report.until.as_deref().unwrap_or("now");
    let kinds: Vec<&str> = report.kinds.iter().map(|kind| kind.as_str()).collect();
    let _ = writeln!(md, "Window: {} to {}  ", inline(since), inline(until));
    let _ = writeln!(
        md,
        "Filters: project={}, tool={}, kinds={}  ",
        inline(report.project.as_deref().unwrap_or("all")),
        inline(report.tool.as_deref().unwrap_or("all")),
        kinds.join(", ")
    );
    let _ = writeln!(md, "Database: `{}`  ", code_span(&report.db_path));
    let mode = match report.mode {
        ReflectMode::ReportOnly => "report only",
        ReflectMode::ReportAndLlm => "report + LLM",
    };
    let _ = writeln!(md, "Mode: {mode}  ");
    match &report.index {
        Some(index) => {
            let _ = writeln!(
                md,
                "Index: {} files discovered, {} new records, {} duplicates skipped, {} parse errors",
                index.discovered_files, index.ingested, index.skipped_dupes, index.parse_errors
            );
        }
        None => md.push_str("Index: skipped (--no-index)\n"),
    }
    if let Some(reason) = &report.llm_fallback_reason {
        let _ = writeln!(md, "\n> LLM skipped: {}", inline(reason));
    }
    md.push('\n');
}

fn write_summary(md: &mut String, report: &ReflectReport) {
    md.push_str("## Summary\n\n");
    if report.assessed.is_empty() && report.unassessed.is_empty() {
        md.push_str("No skill, MCP, or hook incidents found in this window.\n\n");
        return;
    }
    md.push_str("| Kind | Listed | Total | Critical | High | Medium | Low |\n");
    md.push_str("|------|--------|-------|----------|------|--------|-----|\n");
    for row in &report.summary {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} |",
            row.kind.as_str(),
            row.listed,
            row.total,
            row.critical,
            row.high,
            row.medium,
            row.low
        );
    }
    md.push('\n');
    for row in report.summary.iter().filter(|row| row.truncated) {
        let _ = writeln!(
            md,
            "> {}: showing the top {} of {} incidents; the detection window was capped. \
             Use a narrow --since for complete results.",
            row.kind.as_str(),
            row.listed,
            row.total
        );
    }
    md.push_str(
        "Scores are heuristic. They share one formula shape across kinds but are not calibrated between them.\n\n",
    );
}

fn write_assessed(md: &mut String, position: usize, assessed: &ReflectAssessed) {
    let incident = &assessed.incident;
    let _ = writeln!(
        md,
        "## {position}. {} `{}` ({}, score {:.1})\n",
        incident.kind.as_str(),
        code_span(&incident.target),
        inline(&incident.priority_label),
        incident.priority_score
    );
    let _ = writeln!(md, "- Incident: `{}`", code_span(&incident.incident_id));
    let _ = writeln!(
        md,
        "- Session: `{}` ({}, {})",
        code_span(&incident.session_id),
        inline(&incident.tool),
        inline(&incident.project)
    );
    let _ = writeln!(md, "- Last seen: {}", inline(&incident.last_seen));
    if !incident.signals_present.is_empty() {
        let signals: Vec<String> = incident.signals_present.iter().map(|s| inline(s)).collect();
        let _ = writeln!(md, "- Signals: {}", signals.join(", "));
    }
    if let Some(failure) = &assessed.failure {
        let _ = writeln!(md, "- Assessment failed: {}", inline(failure));
    }
    md.push('\n');
    match &assessed.assessment {
        Some(text) => {
            md.push_str(&demote_headings(&block(text)));
            if !md.ends_with('\n') {
                md.push('\n');
            }
        }
        None if assessed.findings.is_null() => {}
        None => {
            let findings = serde_json::to_string_pretty(&assessed.findings)
                .unwrap_or_else(|_| "{}".to_string());
            let _ = writeln!(md, "```json\n{}\n```", block(&findings));
        }
    }
    md.push('\n');
}

fn write_unassessed(md: &mut String, report: &ReflectReport) {
    if report.unassessed.is_empty() {
        return;
    }
    md.push_str("## Other incidents\n\n");
    md.push_str("| Kind | Target | Severity | Score | Last seen | Assess with |\n");
    md.push_str("|------|--------|----------|-------|-----------|-------------|\n");
    for incident in &report.unassessed {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {:.1} | {} | `{}` |",
            incident.kind.as_str(),
            cell(&incident.target),
            cell(&incident.priority_label),
            incident.priority_score,
            cell(&incident.last_seen),
            cell(&incident.assess_command(report.since.as_deref(), report.until.as_deref()))
        );
    }
    md.push('\n');
}

#[cfg(test)]
#[path = "reflect_report_tests.rs"]
mod tests;
