//! Atomic Agent Observatory projector persistence.

#[path = "agent_observatory_projection_types.rs"]
mod types;
pub(super) use types::AgentProjectionWriteFault;
pub use types::{
    AgentActorRow, AgentActorUpsert, AgentProjectionOutboxInput, AgentProjectionOutboxRow,
    AgentProjectionWriteInput, AgentProjectionWriteResult, AgentRunEventUpsert, AgentRunUpsert,
    AgentWorktreeEvidenceUpsert,
};

#[path = "agent_observatory_projection_counters.rs"]
mod counters;
#[path = "agent_observatory_projection_sql.rs"]
mod sql;

use crate::agent_observatory::identity::{actor_key, canonical_tool, event_key, run_key};
use crate::db::pool::{DbPool, write_lock};
use anyhow::{Context, Result, bail};
use rusqlite::TransactionBehavior;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn required(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must be non-empty");
    }
    Ok(())
}

fn timestamp(value: &str, field: &str) -> Result<()> {
    chrono::DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid {field}: {value}"))?;
    Ok(())
}

fn optional_timestamp(value: Option<&str>, field: &str) -> Result<()> {
    if let Some(value) = value {
        timestamp(value, field)?;
    }
    Ok(())
}

fn json_object(value: &str, field: &str) -> Result<()> {
    let parsed: Value =
        serde_json::from_str(value).with_context(|| format!("{field} must be valid JSON"))?;
    if !parsed.is_object() {
        bail!("{field} must be a JSON object");
    }
    Ok(())
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn optional_object_id(value: Option<&str>, field: &str) -> Result<()> {
    if value.is_some_and(|value| !valid_object_id(value)) {
        bail!("{field} must be a 40- or 64-byte hex object ID");
    }
    Ok(())
}

fn validate_run(input: &AgentRunUpsert) -> Result<()> {
    run_key(&input.hostname, &input.tool, &input.native_session_id)?;
    required(&input.status_reason, "status_reason")?;
    timestamp(&input.status_observed_at, "status_observed_at")?;
    timestamp(&input.started_at, "started_at")?;
    timestamp(&input.last_activity_at, "last_activity_at")?;
    optional_timestamp(input.ended_at.as_deref(), "ended_at")?;
    if input.projection_version <= 0 {
        bail!("projection_version must be positive");
    }
    optional_object_id(input.start_head_sha.as_deref(), "start_head_sha")?;
    optional_object_id(input.current_head_sha.as_deref(), "current_head_sha")?;
    json_object(&input.freshness_json, "freshness_json")?;
    json_object(&input.metadata_json, "run metadata_json")?;
    for (field, value) in [
        ("parent_run_key", input.parent_run_key.as_deref()),
        ("previous_run_key", input.previous_run_key.as_deref()),
        (
            "primary_worktree_key",
            input.primary_worktree_key.as_deref(),
        ),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            bail!("{field} must be non-empty when present");
        }
    }
    Ok(())
}

fn validate_actor(input: &AgentActorUpsert) -> Result<()> {
    required(&input.native_actor_id, "native_actor_id")?;
    optional_timestamp(input.started_at.as_deref(), "actor started_at")?;
    optional_timestamp(input.last_activity_at.as_deref(), "actor last_activity_at")?;
    optional_timestamp(input.ended_at.as_deref(), "actor ended_at")?;
    json_object(&input.metadata_json, "actor metadata_json")
}

fn validate_evidence(input: &AgentWorktreeEvidenceUpsert) -> Result<()> {
    required(&input.worktree_key, "evidence worktree_key")?;
    required(&input.evidence_kind, "evidence_kind")?;
    required(&input.evidence_source, "evidence_source")?;
    if !input.confidence.is_finite() || !(0.0..=1.0).contains(&input.confidence) {
        bail!("confidence must be between 0.0 and 1.0");
    }
    timestamp(&input.first_seen_at, "evidence first_seen_at")?;
    timestamp(&input.last_seen_at, "evidence last_seen_at")?;
    json_object(&input.metadata_json, "evidence metadata_json")
}

fn validate_event(input: &AgentRunEventUpsert) -> Result<()> {
    event_key(
        &input.source_kind,
        &input.source_id,
        &input.projection_variant,
    )?;
    if input
        .worktree_key
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("event worktree_key must be non-empty when present");
    }
    timestamp(&input.observed_at, "event observed_at")?;
    timestamp(&input.ingested_at, "event ingested_at")?;
    if input.source_log_id.is_some_and(|value| value <= 0) {
        bail!("source_log_id must be positive when present");
    }
    if input.provider_sequence.is_some_and(|value| value < 0) {
        bail!("provider_sequence must be non-negative when present");
    }
    required(&input.severity, "severity")?;
    json_object(&input.payload_json, "event payload_json")
}

fn validate_outbox(input: &AgentProjectionOutboxInput) -> Result<()> {
    timestamp(&input.expires_at, "outbox expires_at")?;
    json_object(&input.payload_json, "outbox payload_json")
}

fn validate_input(input: &AgentProjectionWriteInput) -> Result<()> {
    validate_run(&input.run)?;
    if let Some(actor) = &input.actor {
        validate_actor(actor)?;
    }
    if let Some(evidence) = &input.worktree_evidence {
        validate_evidence(evidence)?;
    }
    validate_event(&input.event)?;
    validate_outbox(&input.outbox)
}

fn digest_key(namespace: &str, values: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("v1:{namespace}:{:x}", hasher.finalize())
}

fn evidence_key(run_key: &str, input: &AgentWorktreeEvidenceUpsert) -> String {
    digest_key(
        "run_worktree_evidence",
        &[
            run_key,
            &input.worktree_key,
            &input.evidence_kind,
            &input.evidence_source,
        ],
    )
}

fn outbox_key(input: &AgentProjectionWriteInput) -> Result<String> {
    let bytes = serde_json::to_vec(input).context("serialize projection input fingerprint")?;
    Ok(format!("v1:projection_outbox:{:x}", Sha256::digest(bytes)))
}

fn write_inner(
    pool: &DbPool,
    input: &AgentProjectionWriteInput,
    fault: Option<AgentProjectionWriteFault>,
) -> Result<AgentProjectionWriteResult> {
    validate_input(input)?;
    let canonical_tool = canonical_tool(&input.run.tool)?;
    let durable_run_key = run_key(
        &input.run.hostname,
        &canonical_tool,
        &input.run.native_session_id,
    )?;
    let durable_actor_key = input
        .actor
        .as_ref()
        .map(|actor| actor_key(&durable_run_key, &actor.native_actor_id))
        .transpose()?;
    let durable_event_key = event_key(
        &input.event.source_kind,
        &input.event.source_id,
        &input.event.projection_variant,
    )?;
    let durable_evidence_key = input
        .worktree_evidence
        .as_ref()
        .map(|evidence| evidence_key(&durable_run_key, evidence));

    let _write_guard = write_lock();
    let mut connection = pool.get().context("acquire database connection")?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let refs = sql::resolve_run_refs(&tx, &input.run)?;
    let (mut run, run_changed) =
        sql::upsert_run(&tx, &durable_run_key, &canonical_tool, &input.run, &refs)?;

    let (actor, actor_changed) = match (&input.actor, durable_actor_key.as_deref()) {
        (Some(actor), Some(key)) => {
            let (row, changed) = sql::upsert_actor(&tx, key, run.id, actor)?;
            (Some(row), changed)
        }
        (None, None) => (None, false),
        _ => unreachable!("actor input and key are constructed together"),
    };

    let (worktree_evidence, evidence_changed) =
        match (&input.worktree_evidence, durable_evidence_key.as_deref()) {
            (Some(evidence), Some(key)) => {
                let worktree_id = sql::worktree_id(&tx, &evidence.worktree_key)?;
                let (row, changed) = sql::upsert_evidence(&tx, key, run.id, worktree_id, evidence)?;
                (Some(row), changed)
            }
            (None, None) => (None, false),
            _ => unreachable!("evidence input and key are constructed together"),
        };

    let event_worktree_id = input
        .event
        .worktree_key
        .as_deref()
        .map(|key| sql::worktree_id(&tx, key))
        .transpose()?;
    let (event, event_inserted) = sql::insert_event(
        &tx,
        &durable_event_key,
        run.id,
        actor.as_ref().map(|row| row.id),
        event_worktree_id,
        &input.event,
    )?;

    if fault == Some(AgentProjectionWriteFault::AfterEventInsert) {
        bail!("injected failure after event insert");
    }

    if event_inserted {
        run = counters::apply_event_counters(&tx, run.id, &event)?;
    }
    let materialized_state_changed =
        run_changed || actor_changed || evidence_changed || event_inserted;
    let outbox = if materialized_state_changed {
        Some(sql::insert_outbox(
            &tx,
            &outbox_key(input)?,
            run.id,
            &input.outbox,
        )?)
    } else {
        None
    };
    tx.commit()?;

    Ok(AgentProjectionWriteResult {
        run,
        actor,
        worktree_evidence,
        event,
        event_inserted,
        materialized_state_changed,
        outbox,
    })
}

pub fn write_agent_projection(
    pool: &DbPool,
    input: &AgentProjectionWriteInput,
) -> Result<AgentProjectionWriteResult> {
    write_inner(pool, input, None)
}

#[cfg(test)]
pub(super) fn write_agent_projection_with_fault(
    pool: &DbPool,
    input: &AgentProjectionWriteInput,
    fault: AgentProjectionWriteFault,
) -> Result<AgentProjectionWriteResult> {
    write_inner(pool, input, Some(fault))
}

#[cfg(test)]
#[path = "agent_observatory_projection_tests.rs"]
mod tests;
