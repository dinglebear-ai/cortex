use super::*;
use crate::mcp;

const CURRENT_API_ROUTES: &[&str] = &[
    "/api/search",
    "/api/filter",
    "/api/feed",
    "/api/tail",
    "/api/errors",
    "/api/hosts",
    "/api/correlate",
    "/api/stats",
    "/api/version",
    "/api/source-ips",
    "/api/timeline",
    "/api/patterns",
    "/api/ingest-rate",
    "/api/get",
    "/api/host-state",
    "/api/context",
    "/api/fleet-state",
    "/api/correlate-state",
    "/api/topic-correlate",
    "/api/errors/unaddressed",
    "/api/errors/ack",
    "/api/errors/unack",
    "/api/notifications/recent",
    "/api/notifications/test",
    "/api/file-tails",
    "/api/silent-hosts",
    "/api/clock-skew",
    "/api/anomalies",
    "/api/compare",
    "/api/apps",
    "/api/similar-incidents",
    "/api/incident-context",
    "/api/graph/entity",
    "/api/graph/around",
    "/api/graph/explain",
    "/api/graph/evidence",
    "/api/sessions/incidents",
    "/api/sessions/investigate",
    "/api/sessions/llm-invocations",
    "/api/sessions/skills",
    "/api/sessions/skill-incidents",
    "/api/sessions/skill-investigate",
    "/api/sessions/mcp-events",
    "/api/sessions/mcp-incidents",
    "/api/sessions/mcp-investigate",
    "/api/sessions/hooks",
    "/api/sessions/hook-incidents",
    "/api/sessions/hook-investigate",
    "/api/compose/status",
    "/api/compose/doctor",
    "/api/sessions",
    "/api/sessions/search",
    "/api/sessions/abuse",
    "/api/sessions/correlate",
    "/api/sessions/blocks",
    "/api/sessions/context",
    "/api/sessions/tools",
    "/api/sessions/projects",
    "/api/sessions/checkpoints",
    "/api/sessions/errors",
    "/api/sessions/prune-checkpoints",
    "/api/db/status",
    "/api/db/integrity",
    "/api/db/integrity/background",
    "/api/db/integrity/jobs/{id}",
    "/api/db/checkpoint",
    "/api/db/vacuum",
    "/api/db/backup",
];

#[test]
fn every_current_mcp_action_is_classified() {
    for name in mcp::action_names() {
        assert!(
            find(SurfaceKind::McpAction, name).is_some(),
            "MCP action {name} is missing from surfaces catalog"
        );
    }
}

#[test]
fn mcp_access_metadata_matches_scope_gate() {
    for spec in specs_for(SurfaceKind::McpAction) {
        let expected_access = match mcp::required_scope_for(spec.spelling) {
            Some("cortex:read") => SurfaceAccess::Read,
            Some("cortex:admin") => SurfaceAccess::Admin,
            None => SurfaceAccess::Info,
            other => panic!("unexpected MCP scope for {}: {other:?}", spec.spelling),
        };
        assert_eq!(
            spec.access, expected_access,
            "{} registry access must match ACTION_SPECS scope gate",
            spec.spelling
        );
    }
}

#[test]
fn every_current_api_route_is_classified() {
    for route in CURRENT_API_ROUTES {
        assert!(
            find(SurfaceKind::ApiRoute, route).is_some(),
            "API route {route} is missing from surfaces catalog"
        );
    }
}

#[test]
fn retained_operational_roots_are_not_grouped_domains() {
    for root in ["db", "compose", "setup", "config", "doctor", "serve", "mcp"] {
        let spec = find(SurfaceKind::Cli, root).expect("operational root classified");
        assert_eq!(
            spec.disposition,
            SurfaceDisposition::RetainedTopLevelOperational,
            "{root} must stay an operational top-level command"
        );
        assert!(spec.transports.contains(SurfaceTransport::LOCAL_ONLY));
    }
}

#[test]
fn every_removed_cli_spelling_has_one_replacement() {
    let removed: Vec<_> = specs_for(SurfaceKind::Cli)
        .filter(|spec| spec.disposition == SurfaceDisposition::RemovedCleanBreak)
        .collect();
    assert!(!removed.is_empty());
    for spec in removed {
        assert!(
            spec.replacement.is_some(),
            "{} needs a replacement",
            spec.spelling
        );
        assert!(spec.reason.is_some(), "{} needs a reason", spec.spelling);
    }
}

#[test]
fn api_ai_routes_are_intentional_clean_breaks() {
    for route in [
        "/api/ai",
        "/api/ai/search",
        "/api/ai/abuse",
        "/api/ai/correlate",
        "/api/ai/blocks",
        "/api/ai/context",
        "/api/ai/tools",
        "/api/ai/projects",
    ] {
        let spec = find(SurfaceKind::ApiRoute, route).expect("removed /api/ai route");
        assert_eq!(spec.disposition, SurfaceDisposition::RemovedCleanBreak);
        assert!(
            spec.replacement
                .expect("replacement")
                .starts_with("/api/sessions")
        );
    }
}

#[test]
fn all_entries_record_transport_and_access() {
    for kind in [
        SurfaceKind::Cli,
        SurfaceKind::McpAction,
        SurfaceKind::ApiRoute,
    ] {
        for spec in specs_for(kind) {
            match spec.kind {
                SurfaceKind::Cli => assert!(
                    spec.transports.contains(SurfaceTransport::LOCAL_CLI),
                    "{} CLI row lacks local CLI transport",
                    spec.spelling
                ),
                SurfaceKind::McpAction => assert!(
                    spec.transports.contains(SurfaceTransport::MCP),
                    "{} MCP row lacks MCP transport",
                    spec.spelling
                ),
                SurfaceKind::ApiRoute => assert!(
                    spec.transports.contains(SurfaceTransport::REST),
                    "{} API row lacks REST transport",
                    spec.spelling
                ),
            }
            let _ = spec.access;
        }
    }
}
