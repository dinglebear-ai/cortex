#[test]
fn ci_uses_changed_path_classifier_and_stable_gate() {
    let workflow = include_str!("../.github/workflows/ci.yml");

    assert!(
        workflow.contains("scripts/ci/changed_paths.py"),
        "CI must run the changed-path classifier"
    );
    assert!(
        workflow.contains("git show \"${{ github.event.pull_request.base.sha }}:$classifier\""),
        "PRs must use the base-branch classifier when available"
    );

    for job in [
        "version-sync",
        "fmt",
        "clippy",
        "test",
        "docs-contract",
        "coverage",
        "deny",
        "mcp-integration",
    ] {
        let block = workflow_job_block(workflow, job);
        assert!(
            block.contains("needs: [changes"),
            "{job} must depend on the changes job"
        );
        assert!(
            block.contains("needs.changes.outputs."),
            "{job} must be gated by changed-path outputs"
        );
    }

    let docs_contract = workflow_job_block(workflow, "docs-contract");
    assert!(
        docs_contract.contains("needs.changes.outputs.docs == 'true'")
            && docs_contract.contains("cargo test --lib --locked docs_tests::")
            && docs_contract.contains("bash scripts/check-public-identity.sh"),
        "docs-contract must run focused embedded-document tests for docs changes"
    );

    let gate = workflow_job_block(workflow, "ci-gate");
    for job in [
        "changes",
        "version-sync",
        "fmt",
        "clippy",
        "test",
        "docs-contract",
        "coverage",
        "deny",
        "mcp-integration",
    ] {
        assert!(
            gate.contains(&format!("require_success_or_skipped {job}")),
            "ci-gate must cover {job}"
        );
    }

    // The dedicated gitleaks job was retired in favour of the CodeQL/secret
    // scanning already running on the repository; assert it stays gone rather
    // than leaving a stale contract for a job that no longer exists.
    assert!(
        !workflow.contains("gitleaks"),
        "the redundant gitleaks job was retired — re-adding it needs a deliberate contract update"
    );
}

/// Workflows that legitimately need a writable `GITHUB_TOKEN` at the top level.
/// Everything else must declare read-only `contents`. Keeping this as an
/// allowlist means a newly added workflow fails the test until it is either
/// made read-only or consciously added here.
const WRITE_SCOPED_WORKFLOWS: &[&str] = &["release.yml", "openwiki-update.yml"];

#[test]
fn workflows_default_to_read_only_github_token_permissions() {
    // Read the directory at test time rather than `include_str!`ing a fixed
    // list, so a new workflow with no top-level `permissions:` block is caught.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    let mut checked = 0;

    for entry in std::fs::read_dir(&dir).expect("workflows directory must exist") {
        let path = entry.expect("readable workflow entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.ends_with(".yml") && !name.ends_with(".yaml") {
            continue;
        }
        checked += 1;

        let workflow = std::fs::read_to_string(&path).expect("readable workflow");
        let permissions = workflow
            .lines()
            .position(|line| matches!(line.trim_end(), "permissions:" | "permissions: {}"))
            .map(|idx| {
                workflow
                    .lines()
                    .skip(idx + 1)
                    .take_while(|line| line.starts_with("  ") || line.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            });

        let permissions = permissions.unwrap_or_else(|| {
            panic!("{name} must declare an explicit top-level permissions block")
        });

        if WRITE_SCOPED_WORKFLOWS.contains(&name.as_str()) {
            continue;
        }
        assert!(
            workflow
                .lines()
                .any(|line| line.trim_end() == "permissions: {}")
                || permissions.contains("contents: read"),
            "{name} must deny all GITHUB_TOKEN permissions or default contents access to read-only"
        );
    }

    assert!(
        checked > 0,
        "no workflows were checked — is the path right?"
    );

    let docker = include_str!("../.github/workflows/docker-publish.yml");
    let container = workflow_job_block(docker, "container");
    assert!(
        container.contains("hosted-container-release.yml"),
        "the container job must remain delegated to the hardened reusable release workflow"
    );
    assert!(
        docker.contains("packages: write")
            && docker.contains("attestations: write")
            && docker.contains("id-token: write"),
        "the container release workflow must retain package, provenance, and OIDC scopes"
    );
}

#[test]
fn release_please_opens_prs_and_fixes_up_regex_carriers() {
    let release_please = include_str!("../.github/workflows/release-please.yml");
    let release = include_str!("../.github/workflows/release.yml");

    assert!(
        release_please.contains("googleapis/release-please-action"),
        "release-please.yml must run the googleapis release-please-action"
    );
    assert!(
        release_please.contains("config-file: release-please-config.json")
            && release_please.contains("manifest-file: .release-please-manifest.json"),
        "release-please.yml must point at the repo's config/manifest files"
    );
    assert!(
        release_please.contains("RELEASE_PLEASE_TOKEN"),
        "release-please must require a non-default token so it can trigger downstream workflows"
    );
    assert!(
        release_please.contains("cargo xtask sync-version")
            && release_please.contains("cargo xtask check-release-versions"),
        "the release-pr-fixup job must sync regex-based version carriers and re-verify them"
    );

    assert!(
        release.contains("tags: [\"v*\"]"),
        "release.yml must still trigger on the vX.Y.Z tags release-please creates"
    );
    assert!(
        release.contains("publish:") && release.contains("type: boolean"),
        "release workflow must expose an explicit publish input for workflow_dispatch"
    );
    assert!(
        release.contains("github.event.inputs.publish == 'true'"),
        "release publish job must run for tagged workflow_dispatch when publish=true"
    );
    assert!(
        !release.contains("generate_release_notes: true"),
        "release.yml must not overwrite the changelog-derived notes release-please already wrote"
    );
}

#[test]
fn release_please_config_and_manifest_agree_with_components_toml() {
    let config = include_str!("../release-please-config.json");
    let manifest = include_str!("../.release-please-manifest.json");
    let components = include_str!("../release/components.toml");

    assert!(
        config.contains("\"release-type\": \"rust\""),
        "cortex is a single-crate Rust package"
    );
    for extra_file in ["server.json", "mcpb/manifest.json"] {
        assert!(
            config.contains(extra_file),
            "release-please-config.json must declare an extra-files entry for {extra_file}"
        );
    }
    let config_json: serde_json::Value =
        serde_json::from_str(config).expect("release-please config must be valid JSON");
    let extra_files = config_json["packages"]["."]["extra-files"]
        .as_array()
        .expect("root package must declare extra-files");
    for jsonpath in ["$.version", "$.binaryVersion"] {
        assert!(
            extra_files.iter().any(|entry| {
                entry["type"] == "json"
                    && entry["path"] == "packages/cortex-rmcp/package.json"
                    && entry["jsonpath"] == jsonpath
            }),
            "npm launcher {jsonpath} must use an explicit JSON updater"
        );
    }
    assert!(
        !extra_files
            .iter()
            .any(|entry| entry.as_str() == Some("packages/cortex-rmcp/package.json")),
        "package.json must not use the implicit extra-file updater"
    );

    assert!(
        manifest.contains("\".\""),
        ".release-please-manifest.json must track the root package"
    );

    // The two regex_version carriers release-please's extra-files schema
    // can't express must stay declared in components.toml so the
    // release-pr-fixup job's `cargo xtask sync-version` step covers them.
    for regex_carrier in ["server.json", "docker-compose.prod.yml"] {
        assert!(
            components.contains(&format!("regex_version\", path = \"{regex_carrier}\"")),
            "release/components.toml must keep a regex_version carrier for {regex_carrier}"
        );
    }
}

#[test]
fn local_cortex_server_has_auto_deploy_timer_contract() {
    let service = include_str!("../config/systemd/cortex-auto-deploy.service");
    let timer = include_str!("../config/systemd/cortex-auto-deploy.timer");
    let script = include_str!("../scripts/auto-deploy.sh");

    assert!(
        service.contains("ExecStart=") && service.contains("scripts/auto-deploy.sh"),
        "auto-deploy service must execute the repo-owned deploy script"
    );
    assert!(
        timer.contains("OnCalendar=") && timer.contains("cortex-auto-deploy.service"),
        "auto-deploy timer must schedule the service"
    );
    for required in [
        "git fetch --no-tags origin main",
        "git pull --ff-only",
        "docker compose build cortex",
        "docker compose up -d --no-deps --force-recreate cortex",
        "curl -fsS",
        "docker exec cortex cortex --version",
    ] {
        assert!(
            script.contains(required),
            "auto-deploy script must contain {required:?}"
        );
    }
    assert!(
        !script.contains("--tags"),
        "auto-deploy must not fetch tags; stale local tag conflicts must not block deploy"
    );
}

fn workflow_job_block<'a>(workflow: &'a str, job_name: &str) -> &'a str {
    let marker = format!("  {job_name}:");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("job {job_name} exists"));
    let rest = &workflow[start + marker.len()..];
    let end = rest
        .lines()
        .enumerate()
        .skip(1)
        .find_map(|(line_index, line)| {
            let trimmed = line.trim_end();
            if line.starts_with("  ") && !line.starts_with("    ") && trimmed.ends_with(':') {
                let byte_offset = rest
                    .lines()
                    .take(line_index)
                    .map(|line| line.len() + 1)
                    .sum();
                Some(byte_offset)
            } else {
                None
            }
        })
        .unwrap_or(rest.len());
    &rest[..end]
}
