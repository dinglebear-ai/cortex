//! Monotonic display enrichment when a previously unnamed MCP result gains its call.

use anyhow::{Result, bail};
use chrono::{Duration, SecondsFormat, Utc};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::agent_observatory::identity::event_key;
use crate::agent_observatory::projector::mcp_projection_display;
use crate::db::agent_observatory::{
    AgentProjectionOutboxInput, AgentSourceRecord, StreamEventName,
};

pub(crate) fn repair_mcp_result_projection_in_tx(
    tx: &Transaction<'_>,
    source_id: i64,
) -> Result<()> {
    let key = event_key("mcp_events", &source_id.to_string(), "mcp")?;
    let projected: Option<(i64, Option<i64>, i64)> = tx
        .query_row(
            "SELECT id, actor_id, run_id FROM agent_run_events WHERE event_key = ?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((event_id, actor_id, run_id)) = projected else {
        return Ok(());
    };
    let records = super::super::sources::mcp_page(tx, source_id - 1, 1)?;
    let Some(AgentSourceRecord::Mcp(source)) = records.first().filter(
        |record| matches!(record, AgentSourceRecord::Mcp(row) if row.cursor_id == source_id),
    ) else {
        bail!("enriched MCP result source is missing");
    };
    let display = mcp_projection_display(source);
    // Keep immutable event identity, outcomes, counters and source cursors.
    // The existing adapter determines the exact bounded projection payload so
    // ordinary replay continues to pass immutable-event validation.
    let mut changed = tx.execute(
        "UPDATE agent_run_events SET title = ?2, summary = ?3, payload_json = ?4
         WHERE id = ?1 AND (title IS NOT ?2 OR summary IS NOT ?3 OR payload_json IS NOT ?4)",
        params![
            event_id,
            display.title,
            display.summary,
            display.payload_json
        ],
    )? > 0;
    if let Some(actor_id) = actor_id {
        changed |= tx.execute(
            "UPDATE agent_run_actors SET display_name = ?2
             WHERE id = ?1 AND native_actor_id = ?3 AND COALESCE(TRIM(display_name), '') = ''",
            params![
                actor_id,
                display.actor_name,
                format!("mcp:{}", source.call_id)
            ],
        )? > 0;
    }
    if changed {
        let fingerprint = serde_json::to_vec(&(
            event_id,
            &display.title,
            &display.summary,
            &display.payload_json,
            &display.actor_name,
        ))?;
        let correction_key = format!(
            "v1:mcp_identity_enrichment:{:x}",
            Sha256::digest(fingerprint)
        );
        super::sql::insert_outbox(
            tx,
            &correction_key,
            run_id,
            &AgentProjectionOutboxInput {
                event_name: StreamEventName::RunUpdated,
                expires_at: (Utc::now() + Duration::hours(24))
                    .to_rfc3339_opts(SecondsFormat::Millis, true),
                payload_json: json!({
                    "changed": ["mcp_identity"], "event_id": event_id.to_string(),
                    "source_kind": "mcp_events", "source_id": source_id.to_string(),
                })
                .to_string(),
            },
        )?;
    }
    Ok(())
}
