//! Projection adapters for MCP, hook, skill, and LLM source rows.

#[path = "projector_sources_types.rs"]
mod types;
use types::{MAX_SUMMARY_BYTES, ProjectionParts, bounded_payload, projection_parts, truncate_utf8};

use super::super::AGENT_OBSERVATORY_PROJECTION_VERSION;
use crate::agent_observatory::identity::canonical_tool;
use crate::db::DbPool;
use crate::db::agent_observatory::{
    AgentActorUpsert, AgentProjectionOutboxInput, AgentProjectionRunMatch,
    AgentProjectionWriteInput, AgentProjectionWriteResult, AgentRunEventUpsert, AgentRunRow,
    AgentRunUpsert, AgentSourceKind, AgentSourceRecord, AgentWorktreeEvidenceUpsert,
    EvidenceTrustLevel, RunStatus, StreamEventName, find_active_projection_worktree,
    find_unique_projection_run_by_session, write_agent_projection,
};
use anyhow::{Context, Result};
use chrono::{Duration, SecondsFormat, Utc};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceProjectionSkipReason {
    MissingTool,
    MissingSession,
    MissingHostname,
    NoMatchingRun,
    AmbiguousMatchingRun,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProjectionDiagnostic {
    pub source_kind: AgentSourceKind,
    pub source_id: String,
    pub reason: SourceProjectionSkipReason,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceProjectionOutcome {
    Projected(Box<AgentProjectionWriteResult>),
    Skipped(SourceProjectionDiagnostic),
}

fn skip(source: &ProjectionParts, reason: SourceProjectionSkipReason) -> SourceProjectionOutcome {
    SourceProjectionOutcome::Skipped(SourceProjectionDiagnostic {
        source_kind: source.kind,
        source_id: source.source_id.clone(),
        reason,
    })
}

fn expires_at(observed_at: &str) -> Result<String> {
    let observed = chrono::DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid source observed_at: {observed_at}"))?;
    Ok((observed.with_timezone(&Utc) + Duration::hours(24))
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn existing_run_input(row: &AgentRunRow, last_activity_at: &str) -> AgentRunUpsert {
    let last_activity_at = chrono::DateTime::parse_from_rfc3339(last_activity_at)
        .ok()
        .zip(chrono::DateTime::parse_from_rfc3339(&row.last_activity_at).ok())
        .map_or_else(
            || last_activity_at.to_string(),
            |(new, old)| {
                if new > old {
                    last_activity_at.to_string()
                } else {
                    row.last_activity_at.clone()
                }
            },
        );
    AgentRunUpsert {
        native_session_id: row.native_session_id.clone(),
        tool: row.tool.clone(),
        provider_tool: row.provider_tool.clone(),
        hostname: row.hostname.clone(),
        parent_run_key: None,
        previous_run_key: None,
        primary_worktree_key: None,
        transcript_path: row.transcript_path.clone(),
        process_id: row.process_id.clone(),
        status: row.status,
        status_reason: row.status_reason.clone(),
        status_observed_at: row.status_observed_at.clone(),
        started_at: row.started_at.clone(),
        last_activity_at,
        ended_at: row.ended_at.clone(),
        primary_branch: row.primary_branch.clone(),
        start_head_sha: row.start_head_sha.clone(),
        current_head_sha: row.current_head_sha.clone(),
        projection_version: row.projection_version,
        freshness_json: row.freshness_json.clone(),
        metadata_json: row.metadata_json.clone(),
    }
}

fn new_run(
    source: &ProjectionParts,
    tool: &str,
    session: &str,
    hostname: &str,
    worktree_key: Option<&str>,
) -> AgentRunUpsert {
    AgentRunUpsert {
        native_session_id: session.to_string(),
        tool: tool.to_string(),
        provider_tool: source.provider_tool.clone(),
        hostname: hostname.to_string(),
        parent_run_key: None,
        previous_run_key: None,
        primary_worktree_key: worktree_key.map(str::to_string),
        transcript_path: None,
        process_id: None,
        status: RunStatus::Active,
        status_reason: format!("{} activity", source.projection_variant),
        status_observed_at: source.observed_at.clone(),
        started_at: source.observed_at.clone(),
        last_activity_at: source.last_activity_at.clone(),
        ended_at: None,
        primary_branch: None,
        start_head_sha: None,
        current_head_sha: None,
        projection_version: i64::from(AGENT_OBSERVATORY_PROJECTION_VERSION),
        freshness_json: json!({
            "last_source_at": source.observed_at,
            "source_cursor": source.source_cursor,
            "source_kind": source.source_kind,
        })
        .to_string(),
        metadata_json: json!({
            "project": source.project,
            "provider_tool": source.provider_tool,
        })
        .to_string(),
    }
}

fn projection_input(
    pool: &DbPool,
    source: &ProjectionParts,
) -> Result<Result<AgentProjectionWriteInput, SourceProjectionSkipReason>> {
    let Some(raw_tool) = source
        .tool
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Err(SourceProjectionSkipReason::MissingTool));
    };
    let tool = canonical_tool(raw_tool).context("canonicalize source tool")?;
    let Some(session) = source
        .session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Err(SourceProjectionSkipReason::MissingSession));
    };

    let (run, worktree_key, worktree_evidence) = if let Some(hostname) = source.hostname.as_deref()
    {
        if hostname.trim().is_empty() {
            return Ok(Err(SourceProjectionSkipReason::MissingHostname));
        }
        let worktree = match source.project.as_deref() {
            Some(project) if project.starts_with('/') => {
                find_active_projection_worktree(pool, hostname, project)?
            }
            _ => None,
        };
        let worktree_key = worktree.as_ref().map(|row| row.worktree_key.clone());
        let evidence = worktree_key
            .as_ref()
            .map(|key| AgentWorktreeEvidenceUpsert {
                worktree_key: key.clone(),
                evidence_kind: "transcript_project_path".to_string(),
                evidence_source: format!("{}:{}", source.source_kind, source.source_id),
                trust_level: EvidenceTrustLevel::Verified,
                confidence: 0.95,
                is_primary: true,
                first_seen_at: source.observed_at.clone(),
                last_seen_at: source.observed_at.clone(),
                metadata_json: json!({ "project": source.project }).to_string(),
            });
        (
            new_run(source, &tool, session, hostname, worktree_key.as_deref()),
            worktree_key,
            evidence,
        )
    } else {
        match find_unique_projection_run_by_session(pool, &tool, session)? {
            AgentProjectionRunMatch::None => {
                return Ok(Err(SourceProjectionSkipReason::NoMatchingRun));
            }
            AgentProjectionRunMatch::Ambiguous => {
                return Ok(Err(SourceProjectionSkipReason::AmbiguousMatchingRun));
            }
            AgentProjectionRunMatch::Unique(row) => (
                existing_run_input(&row, &source.last_activity_at),
                None,
                None,
            ),
        }
    };

    let payload = bounded_payload(source.payload.clone(), &source.title, &source.source_cursor);
    Ok(Ok(AgentProjectionWriteInput {
        run,
        actor: Some(AgentActorUpsert {
            native_actor_id: source.actor_id.clone(),
            actor_type: Some(source.actor_type.to_string()),
            display_name: Some(truncate_utf8(&source.actor_name, MAX_SUMMARY_BYTES)),
            started_at: Some(source.observed_at.clone()),
            last_activity_at: Some(source.last_activity_at.clone()),
            ended_at: None,
            metadata_json: json!({ "source_kind": source.source_kind }).to_string(),
        }),
        worktree_evidence,
        event: AgentRunEventUpsert {
            source_kind: source.source_kind.to_string(),
            source_id: source.source_id.clone(),
            projection_variant: source.projection_variant.to_string(),
            worktree_key,
            observed_at: source.observed_at.clone(),
            ingested_at: source.observed_at.clone(),
            event_kind: source.event_kind,
            source_log_id: source.source_log_id,
            provider_sequence: source.provider_sequence,
            trace_id: None,
            span_id: None,
            severity: source.severity.clone(),
            title: truncate_utf8(&source.title, MAX_SUMMARY_BYTES),
            summary: source.summary.clone(),
            payload_json: payload,
            content_scrubbed: true,
        },
        outbox: AgentProjectionOutboxInput {
            event_name: StreamEventName::RunEvent,
            expires_at: expires_at(&source.observed_at)?,
            payload_json: json!({
                "event_kind": source.projection_variant,
                "source_cursor": source.source_cursor,
                "source_id": source.source_id,
                "source_kind": source.source_kind,
            })
            .to_string(),
        },
    }))
}

pub fn project_agent_source(
    pool: &DbPool,
    record: &AgentSourceRecord,
) -> Result<SourceProjectionOutcome> {
    let source = projection_parts(record);
    let input = match projection_input(pool, &source)? {
        Ok(input) => input,
        Err(reason) => return Ok(skip(&source, reason)),
    };
    Ok(SourceProjectionOutcome::Projected(Box::new(
        write_agent_projection(pool, &input)?,
    )))
}

#[cfg(test)]
#[path = "projector_sources_tests.rs"]
mod tests;
