//! Parse function for `cortex artifactevents`.
//!
//! The CLI produces the same typed request used by CortexService and REST so
//! local and HTTP modes share validation/query semantics.

use anyhow::{Result, bail};
use cortex::artifact_evidence::ArtifactEvidenceKind;

use super::super::args::{ArtifactEvidenceArgs, CliCommand};
use super::super::{FlagCursor, norm_time, parse_u32_flag};

pub(crate) fn parse_artifact_evidence(args: &[String]) -> Result<CliCommand> {
    let mut parsed = ArtifactEvidenceArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        if arg == "--json" {
            parsed.json = true;
        } else if let Some(value) = flags.match_value(&arg, "--event-kind")? {
            parsed.event_kind = Some(
                value
                    .parse::<ArtifactEvidenceKind>()
                    .map_err(anyhow::Error::msg)?,
            );
        } else if let Some(value) = flags.match_value(&arg, "--artifact-id")? {
            parsed.artifact_id = Some(value);
        } else if let Some(value) = flags.match_value(&arg, "--revision-id")? {
            parsed.revision_id = Some(value);
        } else if let Some(value) = flags.match_value(&arg, "--content-digest")? {
            parsed.content_digest = Some(value);
        } else if let Some(value) = flags.match_value(&arg, "--correlation-id")? {
            parsed.correlation_id = Some(value);
        } else if let Some(value) = flags.match_value(&arg, "--request-id")? {
            parsed.request_id = Some(value);
        } else if let Some(value) = flags.match_value(&arg, "--target-id")? {
            parsed.target_id = Some(value);
        } else if let Some(value) = flags.match_value(&arg, "--source-system")? {
            parsed.source_system = Some(value);
        } else if let Some(value) = flags.match_value(&arg, "--since")? {
            parsed.since = Some(norm_time(value)?);
        } else if let Some(value) = flags.match_value(&arg, "--until")? {
            parsed.until = Some(norm_time(value)?);
        } else if let Some(value) = flags.match_value(&arg, "--limit")? {
            parsed.limit = Some(parse_u32_flag("--limit", value)?);
        } else {
            bail!("unknown artifactevents option: {arg}");
        }
    }
    Ok(CliCommand::ArtifactEvidence(parsed))
}

#[cfg(test)]
#[path = "artifact_evidence_tests.rs"]
mod tests;
