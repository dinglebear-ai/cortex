use super::*;

const UNADDRESSED_SCAN_CAP: usize = 5_000;

/// Presentation-time noise filter for `unaddressed_errors`.
///
/// Distinct from `ErrorDetectionConfig::exclude_patterns`, which drops rows
/// *during scanning* so they are never recorded as signatures. These
/// warning-level probe/health/oauth/policy signatures are still ingested and
/// stay searchable and ack-able — they are only hidden from the unaddressed
/// list. Only `severity == "warning"` rows are eligible; `err+` is never
/// filtered here. Do not fold these patterns back into `exclude_patterns`: the
/// two mechanisms intentionally differ (recorded-but-hidden vs. never-recorded).
/// Matched on the trimmed, lowercased template/sample.
fn is_unaddressed_warning_noise(severity: &str, template: &str, sample_message: &str) -> bool {
    if severity != "warning" {
        return false;
    }

    let template = template.trim().to_ascii_lowercase();
    let sample = sample_message.trim().to_ascii_lowercase();

    let curl_root_probe = template
        .starts_with("get request for '/' received from 127.0.0.1 using 'curl")
        && sample.starts_with("get response status for '/'");
    let oauth_metadata_probe = template.starts_with("get /.well-known/oauth-authorization-server")
        || sample.starts_with("get /.well-known/oauth-authorization-server");
    let health_probe = template.starts_with("get /health => generated")
        || sample.starts_with("get /health => generated");
    let missing_policy_note = matches!(
        template.as_str(),
        "skipping mandatory platform policies because no policy file was found"
            | "skipping recommended platform policies because no policy file was found"
    );
    let labby_tool_list_probe =
        template == "tool list ok" && sample.starts_with("labby tool list ok in ");

    curl_root_probe
        || oauth_metadata_probe
        || health_probe
        || missing_policy_note
        || labby_tool_list_probe
}

impl CortexService {
    // ---- Error detection MCP actions ----------------------------------------

    pub async fn unaddressed_errors(
        &self,
        req: models::UnaddressedErrorsRequest,
    ) -> ServiceResult<models::UnaddressedErrorsResponse> {
        let requested_limit = req.limit.unwrap_or(50).clamp(1, 500) as usize;
        let page_size = requested_limit.saturating_mul(5).clamp(50, 500);
        let include_acked = req.include_acknowledged.unwrap_or(false);
        self.run_db("unaddressed_errors", move |pool| {
            let mut signatures = Vec::with_capacity(requested_limit);
            let mut filtered_count = 0usize;
            let mut candidate_rows = 0usize;
            let mut offset = 0i64;

            while signatures.len() < requested_limit && candidate_rows < UNADDRESSED_SCAN_CAP {
                let remaining_scan = UNADDRESSED_SCAN_CAP - candidate_rows;
                let this_page = page_size.min(remaining_scan);
                let rows = crate::db::error_signatures::read_unaddressed_page(
                    pool,
                    crate::app::error_detection::NORMALIZER_VERSION,
                    this_page as i64,
                    offset,
                    include_acked,
                )?;
                if rows.is_empty() {
                    break;
                }

                candidate_rows += rows.len();
                offset += rows.len() as i64;
                for r in rows {
                    if is_unaddressed_warning_noise(&r.severity, &r.template, &r.sample_message) {
                        filtered_count += 1;
                        continue;
                    }
                    signatures.push(models::ErrorSignatureEntry {
                        signature_hash: r.signature_hash,
                        template: r.template,
                        sample_message: r.sample_message,
                        severity: r.severity,
                        sample_hostname: r.sample_hostname,
                        sample_app_name: r.sample_app_name,
                        first_seen_at: r.first_seen_at,
                        last_seen_at: r.last_seen_at,
                        total_count: r.total_count,
                        count_last_1h: r.count_last_1h,
                        acknowledged_at: r.acknowledged_at,
                    });
                    if signatures.len() == requested_limit {
                        break;
                    }
                }
            }

            Ok(models::UnaddressedErrorsResponse {
                signatures,
                filtered_count,
                candidate_rows,
                candidate_cap: UNADDRESSED_SCAN_CAP,
                candidate_window_truncated: candidate_rows >= UNADDRESSED_SCAN_CAP,
            })
        })
        .await
    }

    pub async fn ack_error(
        &self,
        req: models::AckErrorRequest,
        actor: impl Into<RequestActor>,
    ) -> ServiceResult<models::AckErrorResponse> {
        if let Some(ref n) = req.notes
            && n.len() > 4096
        {
            return Err(ServiceError::InvalidInput(
                "notes exceeds 4096 chars".into(),
            ));
        }
        let hash = req.signature_hash.clone();
        let notes = req.notes.clone();
        let actor = actor.into();
        let actor_owned = actor.display.clone();
        // Check it exists first
        let h = hash.clone();
        let exists = self
            .run_db("ack_error.exists", move |pool| {
                Ok(crate::db::error_signatures::read_signature_by_hash(
                    pool,
                    &h,
                    crate::app::error_detection::NORMALIZER_VERSION,
                )?
                .is_some())
            })
            .await?;
        if !exists {
            return Err(ServiceError::NotFound(format!(
                "Signature '{}' not found",
                hash
            )));
        }
        let now = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let now_clone = now.clone();
        let actor_clone = actor_owned.clone();
        let hash_clone = hash.clone();
        self.run_db("ack_error.commit", move |pool| {
            let mut conn = crate::db::write_conn(pool)?;
            let tx = conn.transaction()?;
            crate::db::error_signatures::record_ack_event(
                &tx,
                &hash_clone,
                crate::app::error_detection::NORMALIZER_VERSION,
                "ack",
                &actor_clone,
                notes.as_deref(),
            )?;
            crate::db::error_signatures::update_ack_projection(
                &tx,
                &hash_clone,
                crate::app::error_detection::NORMALIZER_VERSION,
                Some(&now_clone),
                Some(&actor_clone),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        Ok(models::AckErrorResponse {
            signature_hash: hash,
            acknowledged_at: now,
            actor: actor_owned,
        })
    }

    pub async fn unack_error(
        &self,
        req: models::UnackErrorRequest,
        actor: impl Into<RequestActor>,
    ) -> ServiceResult<models::UnackErrorResponse> {
        if let Some(ref r) = req.reason
            && r.len() > 4096
        {
            return Err(ServiceError::InvalidInput(
                "reason exceeds 4096 chars".into(),
            ));
        }
        let hash = req.signature_hash.clone();
        let reason = req.reason.clone();
        let actor = actor.into();
        let actor_owned = actor.display.clone();
        // Check it exists first
        let h = hash.clone();
        let exists = self
            .run_db("unack_error.exists", move |pool| {
                Ok(crate::db::error_signatures::read_signature_by_hash(
                    pool,
                    &h,
                    crate::app::error_detection::NORMALIZER_VERSION,
                )?
                .is_some())
            })
            .await?;
        if !exists {
            return Err(ServiceError::NotFound(format!(
                "Signature '{}' not found",
                hash
            )));
        }
        let now = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let actor_clone = actor_owned.clone();
        let hash_clone = hash.clone();
        self.run_db("unack_error.commit", move |pool| {
            let mut conn = crate::db::write_conn(pool)?;
            let tx = conn.transaction()?;
            crate::db::error_signatures::record_ack_event(
                &tx,
                &hash_clone,
                crate::app::error_detection::NORMALIZER_VERSION,
                "unack",
                &actor_clone,
                reason.as_deref(),
            )?;
            crate::db::error_signatures::update_ack_projection(
                &tx,
                &hash_clone,
                crate::app::error_detection::NORMALIZER_VERSION,
                None,
                None,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        Ok(models::UnackErrorResponse {
            signature_hash: hash,
            unacked_at: now,
            actor: actor_owned,
        })
    }
}

#[cfg(test)]
#[path = "error_detection_tests.rs"]
mod tests;
