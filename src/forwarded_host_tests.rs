use super::*;
use serde_json::json;

fn proof(key: &str, principal: &str, host: &str) -> Value {
    json!({key: {
        "authenticated_forwarder": principal,
        "transport_peer": "10.1.0.8",
        "hostname_claim": host,
        "trust": if principal == "shared_bearer" { "claimed" } else { "verified_forwarder_claimed_host" },
    }})
}

#[test]
fn shared_credential_and_nat_preserve_distinct_device_claims() {
    for (source, key) in [
        ("agent-ai-transcript://10.1.0.8", "provenance"),
        ("agent-syslog://10.1.0.8", "forwarded_provenance"),
    ] {
        for host in [
            "SERVERHOST",
            "EDGEHOST",
            "workstation.local",
            "edgehost.example.ts.net",
        ] {
            let metadata = proof(key, "shared_bearer", host).to_string();
            assert_eq!(
                subject_hostname("agent-shared_bearer", source, Some(&metadata)),
                Some(host.to_string())
            );
        }
    }
}

#[test]
fn named_device_claim_remains_distinct_from_authentication() {
    let metadata = proof("provenance", "serverhost", "serverhost").to_string();
    assert_eq!(
        subject_hostname(
            "agent-serverhost",
            "agent-ai-transcript://10.1.0.8",
            Some(&metadata)
        ),
        Some("serverhost".to_string())
    );
}

#[test]
fn invalid_placeholder_and_credential_claims_fall_back() {
    for host in [
        "",
        " ",
        "-",
        "unknown",
        "UNKNOWN",
        "localhost",
        "localhost.localdomain",
        "none",
        "[REDACTED]",
        "10.1.0.8",
        "::1",
        "shared_bearer",
        "agent-shared_bearer",
        "bearer-shared-abc",
        "agent-bearer-shared-abc",
        "bad host",
        "bad\nhost",
        "host/path",
        "host:3100",
        "-host",
        "host-",
        "host..local",
        ".host",
        "host_unsafe",
        "💩",
    ] {
        let metadata = proof("provenance", "shared_bearer", host).to_string();
        assert_eq!(
            subject_hostname(
                "agent-shared_bearer",
                "agent-ai-transcript://10.1.0.8",
                Some(&metadata)
            ),
            None,
            "{host:?}"
        );
    }
    for host in ["a".repeat(64), "a.".repeat(127)] {
        let metadata = proof("provenance", "shared_bearer", &host).to_string();
        assert_eq!(
            subject_hostname(
                "agent-shared_bearer",
                "agent-ai-transcript://10.1.0.8",
                Some(&metadata)
            ),
            None
        );
    }
}

#[test]
fn only_matching_server_forwarding_proof_can_attribute_a_row() {
    let metadata = proof("provenance", "shared_bearer", "SERVERHOST");
    let encoded = metadata.to_string();
    for (host, source, value) in [
        (
            "SERVERHOST",
            "agent-ai-transcript://10.1.0.8",
            Some(encoded.as_str()),
        ),
        (
            "agent-other",
            "agent-ai-transcript://10.1.0.8",
            Some(encoded.as_str()),
        ),
        (
            "agent-shared_bearer",
            "10.1.0.8:514",
            Some(encoded.as_str()),
        ),
        (
            "agent-shared_bearer",
            "agent-syslog://10.1.0.8",
            Some(encoded.as_str()),
        ),
        (
            "agent-shared_bearer",
            "agent-ai-transcript://10.1.0.9",
            Some(encoded.as_str()),
        ),
        (
            "agent-shared_bearer",
            "agent-ai-transcript://not-an-ip",
            Some(encoded.as_str()),
        ),
        (
            "agent-shared_bearer",
            "agent-ai-transcript://10.1.0.8",
            None,
        ),
        (
            "agent-shared_bearer",
            "agent-ai-transcript://10.1.0.8",
            Some("not-json"),
        ),
    ] {
        assert_eq!(subject_hostname(host, source, value), None);
    }
    for field in [
        "authenticated_forwarder",
        "transport_peer",
        "hostname_claim",
        "trust",
    ] {
        let mut missing = metadata.clone();
        missing["provenance"].as_object_mut().unwrap().remove(field);
        assert_eq!(
            subject_hostname(
                "agent-shared_bearer",
                "agent-ai-transcript://10.1.0.8",
                Some(&missing.to_string())
            ),
            None
        );
    }
    let mut wrong_trust = metadata;
    wrong_trust["provenance"]["trust"] = json!("verified_forwarder_claimed_host");
    assert_eq!(
        subject_hostname(
            "agent-shared_bearer",
            "agent-ai-transcript://10.1.0.8",
            Some(&wrong_trust.to_string())
        ),
        None
    );
}

#[test]
fn oversized_metadata_keeps_bounded_server_proof() {
    for key in ["provenance", "forwarded_provenance"] {
        let mut metadata = proof(key, "shared_bearer", "EDGEHOST");
        for n in 0..100 {
            metadata[format!("optional-{n}")] = json!("x".repeat(2048));
        }
        metadata["password"] = json!("private-canary");
        metadata["source_type"] = json!("transcript");
        let encoded = bounded_metadata_json(metadata, key);
        assert!(encoded.len() <= crate::ingest_metadata::MAX_METADATA_JSON_BYTES);
        assert!(!encoded.contains("private-canary"));
        let parsed: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(parsed["metadata_truncated"], true);
        assert_eq!(parsed["source_type"], "transcript");
        let source = if key == "provenance" {
            "agent-ai-transcript://10.1.0.8"
        } else {
            "agent-syslog://10.1.0.8"
        };
        assert_eq!(
            subject_hostname("agent-shared_bearer", source, Some(&encoded)),
            Some("EDGEHOST".to_string())
        );
    }
}

#[test]
fn metadata_field_budget_keeps_server_proof() {
    for key in ["provenance", "forwarded_provenance"] {
        let mut metadata = proof(key, "shared_bearer", "SERVERHOST");
        for n in 0..160 {
            metadata[format!("a-optional-{n}")] = json!("small");
        }
        let encoded = bounded_metadata_json(metadata, key);
        let bounded: Value = serde_json::from_str(&encoded).unwrap();
        // Workspace feature unification can preserve insertion order, retaining
        // the proof. Field omission still has the sanitizer's omission marker.
        assert!(
            bounded["metadata_truncated"] == true
                || bounded["_omitted_fields"]
                    .as_u64()
                    .is_some_and(|count| count > 0)
        );
        let source = if key == "provenance" {
            "agent-ai-transcript://10.1.0.8"
        } else {
            "agent-syslog://10.1.0.8"
        };
        assert_eq!(
            subject_hostname("agent-shared_bearer", source, Some(&encoded)),
            Some("SERVERHOST".to_string())
        );
    }
}
