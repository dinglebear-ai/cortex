//! Narrow replay upgrades for historical Codex transcript metadata.

use super::*;

pub(super) fn receipt_key(
    forwarder_identity: &str,
    source_record_id: &str,
    shared_bearer: bool,
) -> String {
    if shared_bearer {
        source_record_id.to_owned()
    } else {
        format!(
            "principal:sha256:{:x}",
            Sha256::digest(format!("{forwarder_identity}\0{source_record_id}").as_bytes())
        )
    }
}

fn prior_fingerprint_matches(
    tx: &rusqlite::Transaction<'_>,
    envelope: &EvidenceEnvelope,
    previous: Option<&str>,
    stored_locator: Option<&str>,
    receipt_key: &str,
) -> anyhow::Result<bool> {
    match previous {
        Some(hash) if hash.starts_with("evidence-v2:sha256:") => {
            v2_fingerprint_matches(envelope, stored_locator, hash)
        }
        Some(hash) if hash.starts_with("evidence-v3:sha256:") => {
            transient_v3_fingerprint_matches(envelope, hash)
        }
        Some(hash) if hash.starts_with("sha256:") => {
            old_fingerprint_matches(tx, receipt_key, envelope, hash)
        }
        None => legacy_receipt_matches(tx, receipt_key, envelope),
        _ => Ok(false),
    }
}

pub(super) fn accept_codex_project_reclassification(
    tx: &rusqlite::Transaction<'_>,
    envelope: &EvidenceEnvelope,
    previous: Option<&str>,
    stored_locator: Option<&str>,
    receipt_key: &str,
    context: &ForwardedEventContext,
) -> anyhow::Result<bool> {
    if envelope.source.provider != "codex" {
        return Ok(false);
    }
    let Some(incoming_project) = envelope.ai_project.as_deref() else {
        return Ok(false);
    };
    let stored_project: Option<String> = tx.query_row(
        "SELECT ai_project FROM logs WHERE id = ?1",
        [context.log_id],
        |row| row.get(0),
    )?;
    let Some(stored_project) = stored_project else {
        return Ok(false);
    };
    if stored_project == incoming_project {
        return Ok(false);
    }

    // A deleted Codex app worktree can no longer supply its .git pointer.
    // Re-scanning the same source line then hashes the raw worktree path
    // instead of the durable project root. Require the previous receipt to
    // prove every other evidence field before accepting this metadata drift.
    let mut original = envelope.clone();
    original.ai_project = Some(stored_project);
    let mut matched = false;
    for clear_events in [false, true] {
        if clear_events {
            original.mcp_events.clear();
            original.hook_events.clear();
        }
        matched = prior_fingerprint_matches(tx, &original, previous, stored_locator, receipt_key)?;
        if matched {
            break;
        }
    }
    if !matched {
        return Ok(false);
    }

    insert_forwarded_events_in_tx(tx, envelope, context)?;
    let fingerprint =
        canonical_v2_fingerprint(envelope, stored_locator)?.ok_or(IdempotencyConflict)?;
    tx.execute(
        "UPDATE ai_transcript_forward_receipts
         SET request_fingerprint = ?2 WHERE source_record_id = ?1",
        rusqlite::params![receipt_key, fingerprint],
    )?;
    // Keep the original canonical project; it was resolved while the
    // worktree existed and is more useful than the now-deleted local path.
    Ok(true)
}

pub(super) fn accept_codex_whitespace_reparse(
    tx: &rusqlite::Transaction<'_>,
    envelope: &EvidenceEnvelope,
    previous: Option<&str>,
    stored_locator: Option<&str>,
    receipt_key: &str,
    context: &ForwardedEventContext,
) -> anyhow::Result<bool> {
    if envelope.source.provider != "codex" {
        return Ok(false);
    }
    let (stored_message, stored_project, metadata_json): (String, Option<String>, Option<String>) =
        tx.query_row(
            "SELECT message, ai_project, metadata_json FROM logs WHERE id = ?1",
            [context.log_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    if stored_message == envelope.message
        || !stored_message
            .split_whitespace()
            .eq(envelope.message.split_whitespace())
    {
        return Ok(false);
    }
    let Some(stored_project) = stored_project else {
        return Ok(false);
    };
    if envelope.ai_project.is_none() {
        return Ok(false);
    }
    let Some(metadata_json) = metadata_json else {
        return Ok(false);
    };
    let metadata: serde_json::Value = serde_json::from_str(&metadata_json)?;
    let stored_kind = metadata
        .get("event_kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let incoming_kind = envelope.event_kind.as_deref().unwrap_or("unknown");
    if stored_kind != incoming_kind
        && !(stored_kind == "unknown" && matches!(incoming_kind, "user" | "assistant"))
    {
        return Ok(false);
    }

    // The same Codex source revision was previously parsed with different
    // whitespace and possibly an unknown speaker. A deleted app worktree may also
    // change its derived project hash. Reconstruct the old envelope and
    // require its exact receipt fingerprint before changing any canonical row.
    let mut original = envelope.clone();
    original.message = stored_message;
    original.ai_project = Some(stored_project);
    original.event_kind = Some(stored_kind.to_string());
    let mut matched = false;
    for clear_events in [false, true] {
        if clear_events {
            original.mcp_events.clear();
            original.hook_events.clear();
        }
        matched = prior_fingerprint_matches(tx, &original, previous, stored_locator, receipt_key)?;
        if matched {
            break;
        }
    }
    if !matched {
        return Ok(false);
    }

    insert_forwarded_events_in_tx(tx, envelope, context)?;
    let fingerprint =
        canonical_v2_fingerprint(envelope, stored_locator)?.ok_or(IdempotencyConflict)?;
    tx.execute(
        "UPDATE ai_transcript_forward_receipts
         SET request_fingerprint = ?2 WHERE source_record_id = ?1",
        rusqlite::params![receipt_key, fingerprint],
    )?;
    tx.execute(
        "UPDATE logs SET message = ?2,
                         metadata_json = json_set(metadata_json, '$.event_kind', ?3)
         WHERE id = ?1",
        rusqlite::params![context.log_id, envelope.message, incoming_kind],
    )?;
    Ok(true)
}

pub(super) fn upgrade_codex_message_role(
    tx: &rusqlite::Transaction<'_>,
    envelope: &EvidenceEnvelope,
    previous: Option<&str>,
    stored_locator: Option<&str>,
    receipt_key: &str,
    context: &ForwardedEventContext,
) -> anyhow::Result<bool> {
    let Some(kind @ ("user" | "assistant")) = envelope.event_kind.as_deref() else {
        return Ok(false);
    };
    if envelope.source.provider != "codex" {
        return Ok(false);
    }

    let mut old = envelope.clone();
    old.event_kind = Some("unknown".to_string());
    let mut matched = false;
    for clear_events in [false, true] {
        if clear_events {
            old.mcp_events.clear();
            old.hook_events.clear();
        }
        matched = prior_fingerprint_matches(tx, &old, previous, stored_locator, receipt_key)?;
        if matched {
            break;
        }
    }
    if !matched {
        return Ok(false);
    }

    // The old fingerprint proves that every other evidence field is equal.
    // Preserve the canonical log and add only the corrected speaker label and
    // any newly available structured events in the same transaction.
    insert_forwarded_events_in_tx(tx, envelope, context)?;
    let fingerprint =
        canonical_v2_fingerprint(envelope, stored_locator)?.ok_or(IdempotencyConflict)?;
    tx.execute(
        "UPDATE ai_transcript_forward_receipts
         SET request_fingerprint = ?2 WHERE source_record_id = ?1",
        rusqlite::params![receipt_key, fingerprint],
    )?;
    tx.execute(
        "UPDATE logs SET metadata_json = json_set(metadata_json, '$.event_kind', ?2)
         WHERE id = ?1",
        rusqlite::params![context.log_id, kind],
    )?;
    Ok(true)
}
