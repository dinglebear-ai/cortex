const CURRENT_DOCKER_DOCS: &[(&str, &str)] = &[
    ("CLAUDE.md", include_str!("../CLAUDE.md")),
    ("README.md", include_str!("../README.md")),
    ("docs/CONFIG.md", include_str!("../docs/CONFIG.md")),
    ("docs/SETUP.md", include_str!("../docs/SETUP.md")),
    (
        "docs/architecture.md",
        include_str!("../docs/architecture.md"),
    ),
    (
        "docs/runbooks/deploy.md",
        include_str!("../docs/runbooks/deploy.md"),
    ),
    ("docs/mcp/ENV.md", include_str!("../docs/mcp/ENV.md")),
    ("docs/SECURITY.md", include_str!("../docs/SECURITY.md")),
];

#[test]
fn current_docker_ingest_docs_prefer_agent_path_over_socket_proxy() {
    for (path, text) in CURRENT_DOCKER_DOCS {
        assert!(
            text.contains("host-local cortex agent")
                || text.contains("host-local agent")
                || text.contains("deployed agent"),
            "{path} should describe the current host-local agent Docker log path"
        );
        assert!(
            text.contains("legacy central pull")
                || text.contains("legacy pull")
                || text.contains("compatibility mode"),
            "{path} should label CORTEX_DOCKER_* as legacy/compatibility coverage"
        );
        assert!(
            !text.contains("Docker socket-proxy ingest"),
            "{path} still presents socket-proxy ingest as a current section heading"
        );
    }
}

#[test]
fn coverage_docs_use_cortex_names_and_current_smoke_scope() {
    let coverage = include_str!("../tests/TEST_COVERAGE.md");
    assert!(
        !coverage.contains("syslog help") && !coverage.contains("syslog status"),
        "tests/TEST_COVERAGE.md should use cortex command/action names after the rebrand"
    );
    for required in [
        "UDP",
        "TCP",
        "file-tail",
        "CLI parity",
        "REST",
        "host-local agent",
        "cargo llvm-cov",
    ] {
        assert!(
            coverage.contains(required),
            "tests/TEST_COVERAGE.md should mention {required}"
        );
    }
}

#[test]
fn coverage_tooling_is_documented_and_scripted() {
    let justfile = include_str!("../Justfile");
    assert!(
        justfile.contains("\ncoverage:") && justfile.contains("cargo llvm-cov nextest"),
        "Justfile should expose a coverage recipe using cargo-llvm-cov + nextest"
    );

    let mcp_tests = include_str!("../docs/mcp/TESTS.md");
    assert!(
        mcp_tests.contains("just coverage") && mcp_tests.contains("cargo llvm-cov"),
        "docs/mcp/TESTS.md should document the coverage workflow"
    );
}

#[test]
fn live_smoke_stabilizes_rollups_before_data_assertions() {
    let live = include_str!("../tests/test_live.sh");
    assert!(
        live.contains("wait_for_timeline_rollup")
            && live.contains("for attempt in {1..120}")
            && live.contains(".rollup_as_of")
            && live.contains("timeline rollup not ready after 120s"),
        "live smoke should give the eager hourly rollup a bounded loaded-CI startup budget"
    );
    assert!(
        live.contains(r#""action":"sessions","project":$project,"since":"2026-05-11T00:00:00Z","until":"2026-05-13T00:00:00Z""#),
        "seeded-session visibility should use an exact time-windowed query instead of a stale rollup"
    );
}

#[test]
fn live_smoke_keeps_deterministic_admin_rest_coverage() {
    let live = include_str!("../tests/test_live.sh");
    assert!(
        live.contains("CORTEX_API_ADMIN_TOKEN"),
        "live smoke should expose a deterministic admin REST gate"
    );
    assert!(
        live.contains("POST /api/file-tails")
            && live.contains(r#"{"op":"status"}"#)
            && live.contains(r#"{"op":"list"}"#),
        "live smoke should cover file-tail status/list admin POST routes"
    );
}
