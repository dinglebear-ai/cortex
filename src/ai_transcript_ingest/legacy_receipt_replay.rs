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
        matched = match previous {
            Some(hash) if hash.starts_with("evidence-v2:sha256:") => {
                v2_fingerprint_matches(&old, stored_locator, hash)?
            }
            Some(hash) if hash.starts_with("evidence-v3:sha256:") => {
                transient_v3_fingerprint_matches(&old, hash)?
            }
            Some(hash) if hash.starts_with("sha256:") => {
                old_fingerprint_matches(tx, receipt_key, &old, hash)?
            }
            None => legacy_receipt_matches(tx, receipt_key, &old)?,
            _ => false,
        };
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
