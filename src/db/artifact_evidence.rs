use anyhow::{Context, Result};
use rusqlite::{
    OptionalExtension, TransactionBehavior, params, params_from_iter, types::Value as SqlValue,
};
use serde::Serialize;
use thiserror::Error;

use crate::artifact_evidence::{
    ARTIFACT_EVIDENCE_APP_NAME, ARTIFACT_EVIDENCE_SYNTHETIC_HOST, ArtifactEvidenceKind,
    NormalizedArtifactEvidence,
};

use super::{DbPool, LogBatchEntry, insert_logs_batch_in_tx, write_lock};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArtifactEvidenceStoreError {
    #[error("artifact evidence eventId already exists for this source with different evidence")]
    EventIdConflict,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactEvidenceAppendResult {
    pub cortex_log_id: i64,
    pub inserted: bool,
    pub event: NormalizedArtifactEvidence,
}

#[derive(Debug, Clone, Default)]
pub struct ArtifactEvidenceParams {
    pub event_kind: Option<ArtifactEvidenceKind>,
    pub artifact_id: Option<String>,
    pub revision_id: Option<String>,
    pub content_digest: Option<String>,
    pub correlation_id: Option<String>,
    pub request_id: Option<String>,
    pub target_id: Option<String>,
    pub source_system: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactEvidenceEntry {
    pub cortex_log_id: i64,
    #[serde(flatten)]
    pub event: NormalizedArtifactEvidence,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListArtifactEvidenceResult {
    pub events: Vec<ArtifactEvidenceEntry>,
    pub truncated: bool,
}

pub fn record_artifact_evidence(
    pool: &DbPool,
    event: NormalizedArtifactEvidence,
) -> Result<ArtifactEvidenceAppendResult> {
    let canonical = serde_json::to_string(&event).context("serialize artifact evidence")?;
    let mut conn = pool.get()?;
    let _write_guard = write_lock();
    // Reserve the SQLite writer slot before the replay lookup. The process-local
    // write lock serializes Cortex writers in this process; IMMEDIATE also closes
    // the check-then-insert race when multiple Cortex processes point at the same
    // SQLite file. A second process cannot pass the eventId lookup until the first
    // process has committed or rolled back.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let existing = tx
        .query_row(
            "SELECT id, metadata_json
             FROM logs
             WHERE app_name = ?1
               AND json_extract(metadata_json, '$.eventId') = ?2
               AND json_extract(metadata_json, '$.sourceSystem') = ?3
               AND json_extract(metadata_json, '$.sourceIssuer') = ?4
             ORDER BY id DESC
             LIMIT 1",
            params![
                ARTIFACT_EVIDENCE_APP_NAME,
                event.event_id,
                event.source_system,
                event.source_issuer
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;

    if let Some((cortex_log_id, stored_json)) = existing {
        let stored: NormalizedArtifactEvidence = serde_json::from_str(&stored_json)
            .context("stored artifact evidence row is malformed")?;
        if stored == event {
            return Ok(ArtifactEvidenceAppendResult {
                cortex_log_id,
                inserted: false,
                event: stored,
            });
        }
        return Err(ArtifactEvidenceStoreError::EventIdConflict.into());
    }

    let summary = event.summary();
    let entry = LogBatchEntry {
        timestamp: event.observed_at.clone(),
        // The canonical logs schema requires a hostname and updates host inventory.
        // Artifact evidence producers are systems/services, not necessarily hosts, so
        // keep them in sourceSystem/sourceIssuer and use one explicit synthetic host
        // rather than polluting homelab host inventory with labby/depot/axon/phoenix.
        hostname: ARTIFACT_EVIDENCE_SYNTHETIC_HOST.to_string(),
        facility: None,
        severity: "info".to_string(),
        app_name: Some(ARTIFACT_EVIDENCE_APP_NAME.to_string()),
        process_id: None,
        message: summary.clone(),
        raw: summary,
        source_ip: event.source_identifier(),
        docker_checkpoint: None,
        ai_tool: None,
        ai_project: None,
        ai_session_id: None,
        ai_transcript_path: None,
        metadata_json: Some(canonical),
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: Some(event.event_kind.as_str().to_string()),
        parse_error: None,
    };

    let ids = insert_logs_batch_in_tx(&tx, std::slice::from_ref(&entry))?;
    let cortex_log_id = *ids
        .first()
        .context("artifact evidence insert did not return a log id")?;
    tx.commit()?;

    Ok(ArtifactEvidenceAppendResult {
        cortex_log_id,
        inserted: true,
        event,
    })
}

pub fn list_artifact_evidence(
    pool: &DbPool,
    params: &ArtifactEvidenceParams,
) -> Result<ListArtifactEvidenceResult> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let mut sql = String::from("SELECT id, metadata_json FROM logs WHERE app_name = ?1");
    let mut values = vec![SqlValue::Text(ARTIFACT_EVIDENCE_APP_NAME.to_string())];

    if let Some(kind) = params.event_kind {
        push_scalar_eq(&mut sql, &mut values, "event_action", kind.as_str());
    }
    push_json_eq(
        &mut sql,
        &mut values,
        "artifactId",
        params.artifact_id.as_deref(),
    );
    push_json_eq(
        &mut sql,
        &mut values,
        "revisionId",
        params.revision_id.as_deref(),
    );
    push_json_eq(
        &mut sql,
        &mut values,
        "contentDigest",
        params.content_digest.as_deref(),
    );
    push_json_eq(
        &mut sql,
        &mut values,
        "correlationId",
        params.correlation_id.as_deref(),
    );
    push_json_eq(
        &mut sql,
        &mut values,
        "requestId",
        params.request_id.as_deref(),
    );
    push_json_eq(
        &mut sql,
        &mut values,
        "targetId",
        params.target_id.as_deref(),
    );
    push_json_eq(
        &mut sql,
        &mut values,
        "sourceSystem",
        params.source_system.as_deref(),
    );
    if let Some(from) = &params.from {
        push_scalar_cmp(&mut sql, &mut values, "timestamp", ">=", from);
    }
    if let Some(to) = &params.to {
        push_scalar_cmp(&mut sql, &mut values, "timestamp", "<=", to);
    }

    values.push(SqlValue::Integer((limit + 1) as i64));
    sql.push_str(&format!(
        " ORDER BY timestamp DESC, id DESC LIMIT ?{}",
        values.len()
    ));

    let conn = pool.get()?;
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(values.iter()))?;
    let mut events = Vec::with_capacity(limit + 1);
    while let Some(row) = rows.next()? {
        let cortex_log_id: i64 = row.get(0)?;
        let metadata_json: String = row.get(1)?;
        let event = serde_json::from_str::<NormalizedArtifactEvidence>(&metadata_json)
            .with_context(|| format!("malformed artifact evidence log row {cortex_log_id}"))?;
        events.push(ArtifactEvidenceEntry {
            cortex_log_id,
            event,
        });
    }
    let truncated = events.len() > limit;
    if truncated {
        events.truncate(limit);
    }
    Ok(ListArtifactEvidenceResult { events, truncated })
}

fn push_scalar_eq(sql: &mut String, values: &mut Vec<SqlValue>, column: &str, value: &str) {
    values.push(SqlValue::Text(value.to_string()));
    sql.push_str(&format!(" AND {column} = ?{}", values.len()));
}

fn push_scalar_cmp(
    sql: &mut String,
    values: &mut Vec<SqlValue>,
    column: &str,
    operator: &str,
    value: &str,
) {
    values.push(SqlValue::Text(value.to_string()));
    sql.push_str(&format!(" AND {column} {operator} ?{}", values.len()));
}

fn push_json_eq(sql: &mut String, values: &mut Vec<SqlValue>, field: &str, value: Option<&str>) {
    let Some(value) = value else { return };
    values.push(SqlValue::Text(value.to_string()));
    sql.push_str(&format!(
        " AND json_extract(metadata_json, '$.{field}') = ?{}",
        values.len()
    ));
}

#[cfg(test)]
#[path = "artifact_evidence_tests.rs"]
mod tests;
