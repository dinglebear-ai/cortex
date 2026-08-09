use super::vocab::*;

#[test]
fn canonical_service_keys_separate_logic_from_topology() {
    assert_eq!(logical_service_key(" Plex "), Some("plex".to_string()));
    assert_eq!(
        service_instance_key("Nashost", " Plex "),
        Some("nashost/plex".to_string())
    );
    assert_eq!(
        split_service_instance_key("nashost/plex"),
        Some(("nashost", "plex"))
    );
}

#[test]
fn container_key_host_extracts_leading_host_segment() {
    assert_eq!(container_key_host("nashost:abc123"), Some("nashost"));
    // Extra colons (e.g. a malformed docker_host) still yield the leading
    // segment as host, matching the container-key construction site which
    // only ever emits a single colon between host and container id.
    assert_eq!(container_key_host("nashost:abc:123"), Some("nashost"));
    assert_eq!(container_key_host("no-colon-key"), None);
    assert_eq!(container_key_host(":abc123"), None);
    assert_eq!(container_key_host("nashost:"), None);
    assert_eq!(container_key_host(""), None);
}

#[test]
fn old_nested_service_shapes_are_classified_not_normalized() {
    assert_eq!(
        classify_legacy_shape("nashost:plex"),
        Some(LegacyShape::HostService)
    );
    assert_eq!(
        classify_legacy_shape("nashost:plex:plex"),
        Some(LegacyShape::HostProjectService)
    );
    assert_eq!(
        classify_legacy_shape("plex/plex/plex"),
        Some(LegacyShape::SlashTriplet)
    );
    assert_eq!(classify_legacy_shape("plex"), None);
    assert_eq!(classify_legacy_shape("nashost/plex"), None);
}

#[test]
fn legacy_shape_classifier_ignores_free_text_and_non_name_segments() {
    let cases: &[(&str, Option<LegacyShape>)] = &[
        // Whitespace anywhere → free text, never a legacy key.
        ("what is nashost:plex doing", None),
        ("plex on nashost: status", None),
        ("a/b/c d", None),
        // Colon shapes where a segment lacks any ASCII alphabetic char.
        ("10.0.0.5:443", None),
        ("12:30", None),
        ("nashost:8080", None),
        (":plex", None),
        ("nashost:", None),
        // Absolute paths are not slash triplets.
        ("/mnt/user/media", None),
        ("/var/lib/docker", None),
        // URLs/URIs are never legacy shapes: a colon segment containing `/`
        // rejects in the colon branch and never falls through to the
        // slash-triplet branch (`https://a.b/c` has two slashes).
        ("http://example.com", None),
        ("https://a.b/c", None),
        ("agent-command://foo", None),
        // Plan-asserted legacy shapes must keep classifying.
        ("nashost:plex", Some(LegacyShape::HostService)),
        ("nashost:plex:plex", Some(LegacyShape::HostProjectService)),
        ("plex/plex/plex", Some(LegacyShape::SlashTriplet)),
        ("a/b/c", Some(LegacyShape::SlashTriplet)),
    ];
    for (input, expected) in cases {
        assert_eq!(
            classify_legacy_shape(input),
            *expected,
            "classify_legacy_shape({input:?})"
        );
    }
}

#[test]
fn key_grammar_edge_cases_pin_non_ascii_empty_and_long_inputs() {
    // Non-ASCII-only input: every char maps to `-`, which trims to nothing.
    assert_eq!(logical_service_key("日本語"), None);
    // Mixed input: ASCII survives, non-ASCII maps to `-` and trims away.
    assert_eq!(logical_service_key("café"), Some("caf".to_string()));
    // Empty / whitespace-only input canonicalizes to nothing.
    assert_eq!(logical_service_key(""), None);
    assert_eq!(logical_service_key("   "), None);
    // Pin current behavior for very long input: there is NO length bound in
    // the key grammar — a >512-char name canonicalizes at full length.
    let long = "a".repeat(600);
    assert_eq!(
        logical_service_key(&long).as_deref().map(str::len),
        Some(600)
    );
}

#[test]
fn canonical_keys_preserve_dots_in_hostnames() {
    let cases: &[(&str, Option<&str>)] = &[
        ("nashost.lan", Some("nashost.lan")),
        ("Nashost.LAN", Some("nashost.lan")),
        (".plex.", Some("plex")),
        ("-.plex.-", Some("plex")),
        ("...", None),
        ("plex media server", Some("plex-media-server")),
    ];
    for (input, expected) in cases {
        assert_eq!(
            logical_service_key(input).as_deref(),
            *expected,
            "logical_service_key({input:?})"
        );
    }
    assert_eq!(
        service_instance_key("nashost.lan", "plex"),
        Some("nashost.lan/plex".to_string())
    );
}

use super::adapters::*;
use super::observation::*;

#[test]
fn agent_docker_identity_extracts_structured_service_instance() {
    let identity = AgentDockerIdentity {
        agent_host: "Nashost".to_string(),
        container_id: "abcdef1234567890".to_string(),
        container_name: "plex".to_string(),
        compose_project: Some("plex".to_string()),
        compose_service: Some("plex".to_string()),
        image: Some("lscr.io/linuxserver/plex:latest".to_string()),
        stream: "stdout".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let observations = observations_from_agent_docker_identity(&identity);
    assert!(observations.iter().any(|o| {
        o.kind == ObservationKind::ServiceInstance
            && o.service_instance_key.as_deref() == Some("nashost/plex")
            && o.logical_service_key.as_deref() == Some("plex")
            && o.trust == ResolverTrust::Verified
            && o.structured
    }));
}

#[test]
fn agent_docker_evidence_path_follows_actual_service_name_source() {
    let with_compose = AgentDockerIdentity {
        agent_host: "nashost".to_string(),
        container_id: "abcdef1234567890".to_string(),
        container_name: "plex-container".to_string(),
        compose_project: Some("plex".to_string()),
        compose_service: Some("plex".to_string()),
        image: None,
        stream: "stdout".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let observations = observations_from_agent_docker_identity(&with_compose);
    assert!(observations.iter().any(|o| {
        o.kind == ObservationKind::LogicalService
            && o.evidence_path == "agent_docker.compose_service"
    }));
    assert!(observations.iter().any(|o| {
        o.kind == ObservationKind::ServiceInstance && o.evidence_path == "agent_docker.host_service"
    }));

    // No compose service label: the logical name fell back to the container
    // name, and the evidence path must say so.
    let without_compose = AgentDockerIdentity {
        compose_project: None,
        compose_service: None,
        ..with_compose
    };
    let observations = observations_from_agent_docker_identity(&without_compose);
    assert!(observations.iter().any(|o| {
        o.kind == ObservationKind::LogicalService
            && o.observed_key == "plex-container"
            && o.evidence_path == "agent_docker.container_name"
    }));
    assert!(observations.iter().any(|o| {
        o.kind == ObservationKind::ServiceInstance && o.evidence_path == "agent_docker.host_service"
    }));
}

#[test]
fn raw_app_label_does_not_create_logical_service_observation_by_itself() {
    let observations = observations_from_raw_app_label(
        "plex/plex/plex",
        "nashost",
        "log",
        "42",
        "2026-01-01T00:00:00Z",
    );
    assert!(
        observations
            .iter()
            .any(|o| o.kind == ObservationKind::RawAppLabel)
    );
    assert!(
        !observations
            .iter()
            .any(|o| o.kind == ObservationKind::LogicalService)
    );
}

#[test]
fn safe_observation_display_redacts_sensitive_values() {
    assert_eq!(
        safe_display_value("https://user:pass@example.test/path"),
        "[redacted]"
    );
    assert_eq!(
        safe_display_value("/home/jmagar/.cortex/token.txt"),
        "[redacted]"
    );
    // Scheme-less credentials must also redact (`:` + `@`, no `://`).
    assert_eq!(
        safe_display_value("user:pass@example.test/path"),
        "[redacted]"
    );
    assert_eq!(safe_display_value("plex"), "plex");
}

use super::resolver::*;

#[test]
fn resolver_converges_duplicate_hosts_under_one_logical_service() {
    let nashost = ResolverObservation {
        kind: ObservationKind::ServiceInstance,
        observed_key: "nashost/plex".to_string(),
        display_label: "nashost/plex".to_string(),
        host_key: Some("nashost".to_string()),
        logical_service_key: Some("plex".to_string()),
        service_instance_key: Some("nashost/plex".to_string()),
        source_kind: "app_inventory".to_string(),
        source_id: "inventory:nashost".to_string(),
        evidence_path: "inventory.services.plex".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
        trust: ResolverTrust::Verified,
        structured: true,
    };
    let backuphost = ResolverObservation {
        service_instance_key: Some("backuphost/plex".to_string()),
        host_key: Some("backuphost".to_string()),
        source_id: "inventory:backuphost".to_string(),
        observed_key: "backuphost/plex".to_string(),
        display_label: "backuphost/plex".to_string(),
        ..nashost.clone()
    };
    let decisions = resolve_observations(&[nashost, backuphost]);
    assert!(
        decisions
            .iter()
            .any(|d| { d.entity_type == ENTITY_TYPE_LOGICAL_SERVICE && d.canonical_key == "plex" })
    );
    assert!(decisions.iter().any(|d| {
        d.entity_type == ENTITY_TYPE_SERVICE_INSTANCE && d.canonical_key == "nashost/plex"
    }));
    assert!(decisions.iter().any(|d| {
        d.entity_type == ENTITY_TYPE_SERVICE_INSTANCE && d.canonical_key == "backuphost/plex"
    }));
}

#[test]
fn mixed_trust_evidence_pins_decision_trust_to_strongest() {
    let verified = ResolverObservation {
        kind: ObservationKind::LogicalService,
        observed_key: "plex".to_string(),
        display_label: "plex".to_string(),
        host_key: None,
        logical_service_key: Some("plex".to_string()),
        service_instance_key: None,
        source_kind: "app_inventory".to_string(),
        source_id: "inventory:nashost".to_string(),
        evidence_path: "inventory.services.name".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
        trust: ResolverTrust::Verified,
        structured: true,
    };
    let inferred = ResolverObservation {
        source_id: "inventory:guess".to_string(),
        trust: ResolverTrust::Inferred,
        ..verified.clone()
    };
    let decisions = resolve_observations(&[verified, inferred]);
    let decision = decisions
        .iter()
        .find(|d| d.entity_type == ENTITY_TYPE_LOGICAL_SERVICE && d.canonical_key == "plex")
        .expect("logical service decision");
    // min() over the strongest-first Ord picks Verified: weak corroborating
    // observations must not weaken independently verified identity.
    assert_eq!(decision.trust, ResolverTrust::Verified);
}

#[test]
fn resolver_rejects_old_key_shapes_before_lookup() {
    for input in ["nashost:plex", "nashost:plex:plex", "plex/plex/plex"] {
        let diagnostic = diagnose_lookup_input(input);
        assert_eq!(diagnostic.status, ResolverStatus::RejectedLegacyShape);
        assert_eq!(diagnostic.reason, "rejected_legacy_shape");
        assert!(diagnostic.candidates.is_empty());
    }
}

#[test]
fn weak_raw_labels_do_not_upgrade_themselves() {
    let observations =
        observations_from_raw_app_label("complex", "nashost", "log", "99", "2026-01-01T00:00:00Z");
    let decisions = resolve_observations(&observations);
    assert!(!decisions.iter().any(|d| d.canonical_key == "plex"));
    assert!(
        !decisions
            .iter()
            .any(|d| d.entity_type == ENTITY_TYPE_LOGICAL_SERVICE)
    );
}

#[test]
fn structured_agent_docker_metadata_resolves_without_central_docker_uri() {
    let identity = AgentDockerIdentity {
        agent_host: "nashost".to_string(),
        container_id: "abcdef1234567890".to_string(),
        container_name: "plex".to_string(),
        compose_project: Some("plex".to_string()),
        compose_service: Some("plex".to_string()),
        image: Some("lscr.io/linuxserver/plex:latest".to_string()),
        stream: "stdout".to_string(),
        observed_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let observations = observations_from_agent_docker_identity(&identity);
    let decisions = resolve_observations(&observations);
    assert!(decisions.iter().any(|d| {
        d.entity_type == ENTITY_TYPE_SERVICE_INSTANCE && d.canonical_key == "nashost/plex"
    }));
}
