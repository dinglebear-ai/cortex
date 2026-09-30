//! Bounded, receipt-checked AI transcript delivery.

use super::*;

pub(super) async fn send_records(
    config: &AiTranscriptForwardConfig,
    client: &reqwest::Client,
    records: Vec<AiTranscriptRecord>,
) -> Result<usize> {
    let sent = records.len();
    let expected_receipts: HashSet<String> = records
        .iter()
        .map(|record| record.envelope.source_record_id.clone())
        .collect();
    anyhow::ensure!(
        expected_receipts.len() == sent,
        "ai transcript forward constructed duplicate source-record identities"
    );
    let empty_request_bytes = serde_json::to_vec(&AiTranscriptIngestRequest {
        records: Vec::new(),
    })?
    .len();
    let mut batch = Vec::new();
    let mut batch_bytes = empty_request_bytes;
    for record in records {
        let record_bytes = serde_json::to_vec(&record)?.len();
        anyhow::ensure!(
            empty_request_bytes + record_bytes <= MAX_FORWARD_BODY_BYTES,
            "one ai transcript evidence record exceeded bounded request budget"
        );
        let separator_bytes = usize::from(!batch.is_empty());
        if batch_bytes + separator_bytes + record_bytes > MAX_FORWARD_BODY_BYTES {
            send_record_batch(config, client, std::mem::take(&mut batch)).await?;
            batch_bytes = empty_request_bytes;
        }
        batch_bytes += usize::from(!batch.is_empty()) + record_bytes;
        batch.push(record);
    }
    if !batch.is_empty() {
        send_record_batch(config, client, batch).await?;
    }
    Ok(sent)
}

async fn send_record_batch(
    config: &AiTranscriptForwardConfig,
    client: &reqwest::Client,
    records: Vec<AiTranscriptRecord>,
) -> Result<()> {
    let sent = records.len();
    let expected_receipts: HashSet<String> = records
        .iter()
        .map(|record| record.envelope.source_record_id.clone())
        .collect();
    let payload = serde_json::to_vec(&AiTranscriptIngestRequest { records })?;
    anyhow::ensure!(
        payload.len() <= MAX_FORWARD_BODY_BYTES,
        "ai transcript forward payload exceeded bounded request budget"
    );
    let mut url = config.target.trim_end_matches('/').to_string();
    url.push_str("/v1/ai-transcripts");
    let mut request = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload);
    if let Some(token) = &config.token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("ai transcript POST failed")?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response
            .text()
            .await
            .map(|body| {
                crate::receiver::enrichment::scrub_ai_message(
                    &truncate_utf8(body.trim(), 1_024),
                    None,
                )
            })
            .unwrap_or_else(|error| format!("response_body_read_failed: {error}"));
        anyhow::bail!("ai transcript forward rejected: {} {}", status, detail);
    }
    let receipt: AiTranscriptIngestResponse = response
        .json()
        .await
        .context("ai transcript forward response was not a receipt")?;
    let returned_receipts: HashSet<String> = receipt
        .receipts
        .iter()
        .map(|receipt| receipt.source_record_id.clone())
        .collect();
    if receipt.accepted != sent
        || receipt.receipts.len() != sent
        || returned_receipts != expected_receipts
    {
        anyhow::bail!(
            "ai transcript forward returned incomplete receipt set: expected {sent}, accepted {}, receipts {}",
            receipt.accepted,
            receipt.receipts.len()
        );
    }

    // Only advance after the server supplied an exact receipt for every
    // submitted source-record ID. A lost/malformed response leaves the local
    // cursor untouched; a retry is deduplicated by the server receipt table.
    Ok(())
}
