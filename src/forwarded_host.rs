//! Device attribution from server-stamped forwarding provenance.
//!
//! This is a claimed device identity, never a new authentication identity.
//! Raw rows, principal receipt namespaces and transport evidence stay intact.

use std::net::IpAddr;

use serde_json::Value;

/// Return a safe device claim only when its server-stamped forwarding lane,
/// principal, trust marker and transport peer agree with the stored row.
pub(crate) fn subject_hostname(
    stored_hostname: &str,
    source_ip: &str,
    metadata_json: Option<&str>,
) -> Option<String> {
    let (peer, provenance_key) =
        if let Some(peer) = source_ip.strip_prefix("agent-ai-transcript://") {
            (peer, "provenance")
        } else {
            let peer = source_ip.strip_prefix("agent-syslog://")?;
            (peer, "forwarded_provenance")
        };
    let peer: IpAddr = peer.parse().ok()?;
    let encoded = metadata_json?;
    if encoded.len() > crate::ingest_metadata::MAX_METADATA_JSON_BYTES {
        return None;
    }
    let metadata: Value = serde_json::from_str(encoded).ok()?;
    let provenance = metadata.get(provenance_key)?.as_object()?;
    let principal = provenance.get("authenticated_forwarder")?.as_str()?;
    if principal.is_empty()
        || principal.len() > 512
        || stored_hostname != format!("agent-{principal}")
        || provenance
            .get("transport_peer")?
            .as_str()?
            .parse::<IpAddr>()
            .ok()?
            != peer
    {
        return None;
    }
    let expected_trust = if principal == "shared_bearer" {
        "claimed"
    } else {
        "verified_forwarder_claimed_host"
    };
    if provenance.get("trust")?.as_str()? != expected_trust {
        return None;
    }
    let claim = provenance.get("hostname_claim")?.as_str()?;
    valid_hostname(claim, principal).then(|| claim.to_owned())
}

fn valid_hostname(claim: &str, principal: &str) -> bool {
    if claim.is_empty() || claim.len() > 253 || !claim.is_ascii() {
        return false;
    }
    let hostname = claim.strip_suffix('.').unwrap_or(claim);
    let lower = hostname.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "unknown"
            | "localhost"
            | "localhost.localdomain"
            | "none"
            | "null"
            | "nil"
            | "undefined"
            | "unavailable"
            | "unspecified"
            | "redacted"
            | "shared-bearer"
            | "shared_bearer"
            | "loopback"
            | "agent-loopback"
            | "agent-shared-bearer"
            | "agent-shared_bearer"
    ) || lower.starts_with("bearer-")
        || lower.starts_with("agent-bearer-")
        || hostname.eq_ignore_ascii_case(&format!("agent-{principal}"))
        || hostname.parse::<IpAddr>().is_ok()
    {
        return false;
    }
    hostname.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

/// Keep device proof available when optional evidence exhausts the metadata
/// budget. The normal sanitizer still bounds and redacts both representations.
pub(crate) fn bounded_metadata_json(metadata: Value, provenance_key: &str) -> String {
    let fallback = serde_json::json!({
        provenance_key: metadata.get(provenance_key),
        "source_type": metadata.get("source_type"),
        "source_kind": metadata.get("source_kind"),
        "evidence_envelope_version": metadata.get("evidence_envelope_version"),
        "content_scrubbed": metadata.get("content_scrubbed"),
        "metadata_truncated": true,
    });
    let encoded = crate::ingest_metadata::bounded_metadata_json(metadata);
    let lost_proof = serde_json::from_str::<Value>(&encoded)
        .ok()
        .is_none_or(|value| value.get(provenance_key).is_none_or(Value::is_null));
    if lost_proof {
        crate::ingest_metadata::bounded_metadata_json(fallback)
    } else {
        encoded
    }
}

#[cfg(test)]
#[path = "forwarded_host_tests.rs"]
mod tests;
