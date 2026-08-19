//! CLI query projection for artifact ecosystem evidence.
//!
//! Local and HTTP modes consume the same app-layer request/response contract.

use anyhow::Result;
use cortex::app::{ListArtifactEvidenceRequest, ListArtifactEvidenceResponse};

use super::super::CliMode;
use super::super::args::ArtifactEvidenceArgs;
use super::super::output::common::print_json;
use super::http_or_cancel;

impl ArtifactEvidenceArgs {
    pub(crate) fn into_request(self) -> ListArtifactEvidenceRequest {
        ListArtifactEvidenceRequest {
            event_kind: self.event_kind,
            artifact_id: self.artifact_id,
            revision_id: self.revision_id,
            content_digest: self.content_digest,
            correlation_id: self.correlation_id,
            request_id: self.request_id,
            target_id: self.target_id,
            source_system: self.source_system,
            from: self.since,
            to: self.until,
            limit: self.limit,
        }
    }
}

pub(crate) async fn run_artifact_evidence(
    mode: &CliMode,
    args: ArtifactEvidenceArgs,
) -> Result<()> {
    let json = args.json;
    let req = args.into_request();
    let response = match mode {
        CliMode::Local(service) => service.list_artifact_evidence(req).await?,
        CliMode::Http(client) => http_or_cancel(client.artifact_evidence(&req)).await?,
    };
    if json {
        return print_json(&response);
    }
    print_human(&response);
    Ok(())
}

fn print_human(response: &ListArtifactEvidenceResponse) {
    println!(
        "{} artifact evidence event(s){}:",
        response.events.len(),
        if response.truncated {
            " (truncated)"
        } else {
            ""
        }
    );
    for entry in &response.events {
        let event = &entry.event;
        let artifact = event.artifact_id.as_deref().unwrap_or("-");
        let revision = event.revision_id.as_deref().unwrap_or("-");
        let target = event.target_id.as_deref().unwrap_or("-");
        let correlation = event.correlation_id.as_deref().unwrap_or("-");
        println!(
            "  {} {:<28} source={:<12} artifact={} revision={} target={} correlation={} log={}",
            event.observed_at,
            event.event_kind.as_str(),
            event.source_system,
            artifact,
            revision,
            target,
            correlation,
            entry.cortex_log_id
        );
    }
}

#[cfg(test)]
#[path = "artifact_evidence_tests.rs"]
mod tests;
