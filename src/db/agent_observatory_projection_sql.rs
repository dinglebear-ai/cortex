//! SQL primitives for one atomic Agent Observatory projection write.

use super::super::{AgentEventKind, AgentRunEventRow, AgentRunRow, AgentRunWorktreeEvidenceRow};
use anyhow::{Context, Result, bail};
use rusqlite::types::Type;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use std::str::FromStr;

use super::types::{
    AgentActorRow, AgentActorUpsert, AgentProjectionOutboxInput, AgentProjectionOutboxRow,
    AgentRunEventUpsert, AgentRunUpsert, AgentWorktreeEvidenceUpsert,
};

const RUN_COLUMNS: &str = "id, run_key, native_session_id, tool, provider_tool, hostname,
 parent_run_id, previous_run_id, primary_worktree_id, transcript_path, process_id, status,
 status_reason, status_observed_at, started_at, last_activity_at, ended_at,
 first_source_log_id, last_source_log_id, last_event_id, event_count, error_count,
 primary_branch, start_head_sha, current_head_sha, projection_version, freshness_json,
 metadata_json, created_at, updated_at";
const ACTOR_COLUMNS: &str = "id, actor_key, run_id, native_actor_id, actor_type, display_name,
 started_at, last_activity_at, ended_at, metadata_json";
const EVIDENCE_COLUMNS: &str = "id, relation_key, run_id, worktree_id, evidence_kind,
 evidence_source, trust_level, confidence, is_primary, first_seen_at, last_seen_at,
 metadata_json";
const EVENT_COLUMNS: &str = "id, event_key, run_id, actor_id, worktree_id, commit_id,
 observed_at, ingested_at, event_kind, source_kind, source_id, source_log_id,
 provider_sequence, trace_id, span_id, severity, title, summary, payload_json,
 content_scrubbed, created_at";
const OUTBOX_COLUMNS: &str = "id, outbox_key, run_id, stream_event_type, expires_at,
 payload_json, created_at";

fn enum_value<T>(row: &Row<'_>, index: usize, _name: &'static str) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let value: String = row.get(index)?;
    value.parse().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn run_row(row: &Row<'_>) -> rusqlite::Result<AgentRunRow> {
    Ok(AgentRunRow {
        id: row.get(0)?,
        run_key: row.get(1)?,
        native_session_id: row.get(2)?,
        tool: row.get(3)?,
        provider_tool: row.get(4)?,
        hostname: row.get(5)?,
        parent_run_id: row.get(6)?,
        previous_run_id: row.get(7)?,
        primary_worktree_id: row.get(8)?,
        transcript_path: row.get(9)?,
        process_id: row.get(10)?,
        status: enum_value(row, 11, "status")?,
        status_reason: row.get(12)?,
        status_observed_at: row.get(13)?,
        started_at: row.get(14)?,
        last_activity_at: row.get(15)?,
        ended_at: row.get(16)?,
        first_source_log_id: row.get(17)?,
        last_source_log_id: row.get(18)?,
        last_event_id: row.get(19)?,
        event_count: row.get(20)?,
        error_count: row.get(21)?,
        primary_branch: row.get(22)?,
        start_head_sha: row.get(23)?,
        current_head_sha: row.get(24)?,
        projection_version: row.get(25)?,
        freshness_json: row.get(26)?,
        metadata_json: row.get(27)?,
        created_at: row.get(28)?,
        updated_at: row.get(29)?,
    })
}

fn actor_row(row: &Row<'_>) -> rusqlite::Result<AgentActorRow> {
    Ok(AgentActorRow {
        id: row.get(0)?,
        actor_key: row.get(1)?,
        run_id: row.get(2)?,
        native_actor_id: row.get(3)?,
        actor_type: row.get(4)?,
        display_name: row.get(5)?,
        started_at: row.get(6)?,
        last_activity_at: row.get(7)?,
        ended_at: row.get(8)?,
        metadata_json: row.get(9)?,
    })
}

fn evidence_row(row: &Row<'_>) -> rusqlite::Result<AgentRunWorktreeEvidenceRow> {
    Ok(AgentRunWorktreeEvidenceRow {
        id: row.get(0)?,
        relation_key: row.get(1)?,
        run_id: row.get(2)?,
        worktree_id: row.get(3)?,
        evidence_kind: row.get(4)?,
        evidence_source: row.get(5)?,
        trust_level: enum_value(row, 6, "trust_level")?,
        confidence: row.get(7)?,
        is_primary: row.get(8)?,
        first_seen_at: row.get(9)?,
        last_seen_at: row.get(10)?,
        metadata_json: row.get(11)?,
    })
}

fn event_row(row: &Row<'_>) -> rusqlite::Result<AgentRunEventRow> {
    Ok(AgentRunEventRow {
        id: row.get(0)?,
        event_key: row.get(1)?,
        run_id: row.get(2)?,
        actor_id: row.get(3)?,
        worktree_id: row.get(4)?,
        commit_id: row.get(5)?,
        observed_at: row.get(6)?,
        ingested_at: row.get(7)?,
        event_kind: enum_value(row, 8, "event_kind")?,
        source_kind: row.get(9)?,
        source_id: row.get(10)?,
        source_log_id: row.get(11)?,
        provider_sequence: row.get(12)?,
        trace_id: row.get(13)?,
        span_id: row.get(14)?,
        severity: row.get(15)?,
        title: row.get(16)?,
        summary: row.get(17)?,
        payload_json: row.get(18)?,
        content_scrubbed: row.get(19)?,
        created_at: row.get(20)?,
    })
}

fn outbox_row(row: &Row<'_>) -> rusqlite::Result<AgentProjectionOutboxRow> {
    Ok(AgentProjectionOutboxRow {
        id: row.get(0)?,
        outbox_key: row.get(1)?,
        run_id: row.get(2)?,
        event_name: enum_value(row, 3, "stream_event_type")?,
        expires_at: row.get(4)?,
        payload_json: row.get(5)?,
        created_at: row.get(6)?,
    })
}

pub(super) fn run_id(tx: &Transaction<'_>, key: &str) -> Result<Option<i64>> {
    tx.query_row(
        "SELECT id FROM agent_runs WHERE run_key = ?1",
        [key],
        |row| row.get(0),
    )
    .optional()
    .context("query run ID")
}

pub(super) fn required_run_id(tx: &Transaction<'_>, key: &str) -> Result<i64> {
    run_id(tx, key)?.with_context(|| format!("run not found for key {key}"))
}

pub(super) fn worktree_id(tx: &Transaction<'_>, key: &str) -> Result<i64> {
    tx.query_row(
        "SELECT id FROM repository_worktrees WHERE worktree_key = ?1 AND removed_at IS NULL",
        [key],
        |row| row.get(0),
    )
    .optional()?
    .with_context(|| format!("worktree not found for key {key}"))
}

pub(super) struct RunRefs {
    pub parent_run_id: Option<i64>,
    pub previous_run_id: Option<i64>,
    pub primary_worktree_id: Option<i64>,
}

pub(super) fn resolve_run_refs(tx: &Transaction<'_>, input: &AgentRunUpsert) -> Result<RunRefs> {
    Ok(RunRefs {
        parent_run_id: input
            .parent_run_key
            .as_deref()
            .map(|key| required_run_id(tx, key))
            .transpose()?,
        previous_run_id: input
            .previous_run_key
            .as_deref()
            .map(|key| required_run_id(tx, key))
            .transpose()?,
        primary_worktree_id: input
            .primary_worktree_key
            .as_deref()
            .map(|key| worktree_id(tx, key))
            .transpose()?,
    })
}

pub(super) fn upsert_run(
    tx: &Transaction<'_>,
    key: &str,
    canonical_tool: &str,
    input: &AgentRunUpsert,
    refs: &RunRefs,
) -> Result<(AgentRunRow, bool)> {
    tx.execute(
        "INSERT INTO agent_runs
            (run_key, native_session_id, tool, provider_tool, hostname, parent_run_id,
             previous_run_id, primary_worktree_id, transcript_path, process_id, status,
             status_reason, status_observed_at, started_at, last_activity_at, ended_at,
             primary_branch, start_head_sha, current_head_sha, projection_version,
             freshness_json, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
         ON CONFLICT(run_key) DO UPDATE SET
             provider_tool=excluded.provider_tool, parent_run_id=excluded.parent_run_id,
             previous_run_id=excluded.previous_run_id,
             primary_worktree_id=excluded.primary_worktree_id,
             transcript_path=excluded.transcript_path, process_id=excluded.process_id,
             status=excluded.status, status_reason=excluded.status_reason,
             status_observed_at=excluded.status_observed_at, started_at=excluded.started_at,
             last_activity_at=excluded.last_activity_at, ended_at=excluded.ended_at,
             primary_branch=excluded.primary_branch, start_head_sha=excluded.start_head_sha,
             current_head_sha=excluded.current_head_sha,
             projection_version=excluded.projection_version,
             freshness_json=excluded.freshness_json, metadata_json=excluded.metadata_json,
             updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE agent_runs.provider_tool IS NOT excluded.provider_tool
            OR agent_runs.parent_run_id IS NOT excluded.parent_run_id
            OR agent_runs.previous_run_id IS NOT excluded.previous_run_id
            OR agent_runs.primary_worktree_id IS NOT excluded.primary_worktree_id
            OR agent_runs.transcript_path IS NOT excluded.transcript_path
            OR agent_runs.process_id IS NOT excluded.process_id
            OR agent_runs.status IS NOT excluded.status
            OR agent_runs.status_reason IS NOT excluded.status_reason
            OR agent_runs.status_observed_at IS NOT excluded.status_observed_at
            OR agent_runs.started_at IS NOT excluded.started_at
            OR agent_runs.last_activity_at IS NOT excluded.last_activity_at
            OR agent_runs.ended_at IS NOT excluded.ended_at
            OR agent_runs.primary_branch IS NOT excluded.primary_branch
            OR agent_runs.start_head_sha IS NOT excluded.start_head_sha
            OR agent_runs.current_head_sha IS NOT excluded.current_head_sha
            OR agent_runs.projection_version IS NOT excluded.projection_version
            OR agent_runs.freshness_json IS NOT excluded.freshness_json
            OR agent_runs.metadata_json IS NOT excluded.metadata_json",
        params![
            key,
            input.native_session_id,
            canonical_tool,
            input.provider_tool,
            input.hostname,
            refs.parent_run_id,
            refs.previous_run_id,
            refs.primary_worktree_id,
            input.transcript_path,
            input.process_id,
            input.status.as_str(),
            input.status_reason,
            input.status_observed_at,
            input.started_at,
            input.last_activity_at,
            input.ended_at,
            input.primary_branch,
            input.start_head_sha,
            input.current_head_sha,
            input.projection_version,
            input.freshness_json,
            input.metadata_json
        ],
    )?;
    let changed = tx.changes() > 0;
    let sql = format!("SELECT {RUN_COLUMNS} FROM agent_runs WHERE run_key = ?1");
    let row = tx.query_row(&sql, [key], run_row)?;
    if row.hostname != input.hostname
        || row.tool != canonical_tool
        || row.native_session_id != input.native_session_id
    {
        bail!("run identity conflict for key {key}");
    }
    Ok((row, changed))
}

pub(super) fn upsert_actor(
    tx: &Transaction<'_>,
    key: &str,
    run_id: i64,
    input: &AgentActorUpsert,
) -> Result<(AgentActorRow, bool)> {
    tx.execute(
        "INSERT INTO agent_run_actors
            (actor_key, run_id, native_actor_id, actor_type, display_name, started_at,
             last_activity_at, ended_at, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(actor_key) DO UPDATE SET actor_type=excluded.actor_type,
             display_name=excluded.display_name, started_at=excluded.started_at,
             last_activity_at=excluded.last_activity_at, ended_at=excluded.ended_at,
             metadata_json=excluded.metadata_json
         WHERE agent_run_actors.actor_type IS NOT excluded.actor_type
            OR agent_run_actors.display_name IS NOT excluded.display_name
            OR agent_run_actors.started_at IS NOT excluded.started_at
            OR agent_run_actors.last_activity_at IS NOT excluded.last_activity_at
            OR agent_run_actors.ended_at IS NOT excluded.ended_at
            OR agent_run_actors.metadata_json IS NOT excluded.metadata_json",
        params![
            key,
            run_id,
            input.native_actor_id,
            input.actor_type,
            input.display_name,
            input.started_at,
            input.last_activity_at,
            input.ended_at,
            input.metadata_json
        ],
    )?;
    let changed = tx.changes() > 0;
    let sql = format!("SELECT {ACTOR_COLUMNS} FROM agent_run_actors WHERE actor_key = ?1");
    let row = tx.query_row(&sql, [key], actor_row)?;
    if row.run_id != run_id || row.native_actor_id != input.native_actor_id {
        bail!("actor identity conflict for key {key}");
    }
    Ok((row, changed))
}

pub(super) fn upsert_evidence(
    tx: &Transaction<'_>,
    key: &str,
    run_id: i64,
    worktree_id: i64,
    input: &AgentWorktreeEvidenceUpsert,
) -> Result<(AgentRunWorktreeEvidenceRow, bool)> {
    tx.execute(
        "INSERT INTO agent_run_worktrees
            (relation_key, run_id, worktree_id, evidence_kind, evidence_source, trust_level,
             confidence, is_primary, first_seen_at, last_seen_at, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(relation_key) DO UPDATE SET trust_level=excluded.trust_level,
             confidence=excluded.confidence, is_primary=excluded.is_primary,
             first_seen_at=MIN(agent_run_worktrees.first_seen_at, excluded.first_seen_at),
             last_seen_at=MAX(agent_run_worktrees.last_seen_at, excluded.last_seen_at),
             metadata_json=excluded.metadata_json
         WHERE agent_run_worktrees.trust_level IS NOT excluded.trust_level
            OR agent_run_worktrees.confidence IS NOT excluded.confidence
            OR agent_run_worktrees.is_primary IS NOT excluded.is_primary
            OR agent_run_worktrees.first_seen_at > excluded.first_seen_at
            OR agent_run_worktrees.last_seen_at < excluded.last_seen_at
            OR agent_run_worktrees.metadata_json IS NOT excluded.metadata_json",
        params![
            key,
            run_id,
            worktree_id,
            input.evidence_kind,
            input.evidence_source,
            input.trust_level.as_str(),
            input.confidence,
            input.is_primary,
            input.first_seen_at,
            input.last_seen_at,
            input.metadata_json
        ],
    )?;
    let changed = tx.changes() > 0;
    let sql = format!("SELECT {EVIDENCE_COLUMNS} FROM agent_run_worktrees WHERE relation_key = ?1");
    let row = tx.query_row(&sql, [key], evidence_row)?;
    if row.run_id != run_id
        || row.worktree_id != worktree_id
        || row.evidence_kind != input.evidence_kind
        || row.evidence_source != input.evidence_source
    {
        bail!("worktree evidence identity conflict for key {key}");
    }
    Ok((row, changed))
}

pub(super) fn insert_event(
    tx: &Transaction<'_>,
    key: &str,
    run_id: i64,
    actor_id: Option<i64>,
    worktree_id: Option<i64>,
    input: &AgentRunEventUpsert,
) -> Result<(AgentRunEventRow, bool)> {
    tx.execute(
        "INSERT INTO agent_run_events
            (event_key, run_id, actor_id, worktree_id, observed_at, ingested_at, event_kind,
             source_kind, source_id, source_log_id, provider_sequence, trace_id, span_id,
             severity, title, summary, payload_json, content_scrubbed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18)
         ON CONFLICT(event_key) DO NOTHING",
        params![
            key,
            run_id,
            actor_id,
            worktree_id,
            input.observed_at,
            input.ingested_at,
            input.event_kind.as_str(),
            input.source_kind,
            input.source_id,
            input.source_log_id,
            input.provider_sequence,
            input.trace_id,
            input.span_id,
            input.severity,
            input.title,
            input.summary,
            input.payload_json,
            input.content_scrubbed
        ],
    )?;
    let inserted = tx.changes() > 0;
    let sql = format!("SELECT {EVENT_COLUMNS} FROM agent_run_events WHERE event_key = ?1");
    let row = tx.query_row(&sql, [key], event_row)?;
    if row.run_id != run_id
        || row.source_kind != input.source_kind
        || row.source_id != input.source_id
        || row.event_kind != input.event_kind
    {
        bail!("event identity conflict for key {key}");
    }
    Ok((row, inserted))
}

pub(super) fn apply_event_counters(
    tx: &Transaction<'_>,
    run_id: i64,
    event: &AgentRunEventRow,
) -> Result<AgentRunRow> {
    let error_increment = i64::from(event.event_kind == AgentEventKind::Error);
    tx.execute(
        "UPDATE agent_runs SET last_event_id=?1, event_count=event_count+1,
             error_count=error_count+?2,
             first_source_log_id=CASE
                 WHEN ?3 IS NULL THEN first_source_log_id
                 WHEN first_source_log_id IS NULL OR first_source_log_id > ?3 THEN ?3
                 ELSE first_source_log_id END,
             last_source_log_id=CASE
                 WHEN ?3 IS NULL THEN last_source_log_id
                 WHEN last_source_log_id IS NULL OR last_source_log_id < ?3 THEN ?3
                 ELSE last_source_log_id END,
             last_activity_at=MAX(last_activity_at, ?4),
             updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?5",
        params![
            event.id,
            error_increment,
            event.source_log_id,
            event.observed_at,
            run_id
        ],
    )?;
    let sql = format!("SELECT {RUN_COLUMNS} FROM agent_runs WHERE id = ?1");
    tx.query_row(&sql, [run_id], run_row)
        .context("query run after event counters")
}

pub(super) fn insert_outbox(
    tx: &Transaction<'_>,
    key: &str,
    run_id: i64,
    input: &AgentProjectionOutboxInput,
) -> Result<AgentProjectionOutboxRow> {
    tx.execute(
        "INSERT INTO agent_stream_outbox
            (outbox_key, run_id, stream_event_type, expires_at, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(outbox_key) DO NOTHING",
        params![
            key,
            run_id,
            input.event_name.as_str(),
            input.expires_at,
            input.payload_json
        ],
    )?;
    let sql = format!("SELECT {OUTBOX_COLUMNS} FROM agent_stream_outbox WHERE outbox_key = ?1");
    tx.query_row(&sql, [key], outbox_row)
        .context("query projection outbox row")
}
