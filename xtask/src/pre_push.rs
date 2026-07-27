use anyhow::{Context, Result};
use clap::Parser;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Parser)]
pub struct PrePushArgs {
    /// Print the selected plan without running commands.
    #[arg(long)]
    pub dry_run: bool,
    /// Read changed paths from a file instead of diffing git.
    #[arg(long)]
    pub changed_files: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct Categories {
    docs: bool,
    docker: bool,
    hooks: bool,
    release: bool,
    rust: bool,
    skills: bool,
    web: bool,
}

impl Categories {
    fn all() -> Self {
        Self {
            docs: true,
            docker: true,
            hooks: true,
            release: true,
            rust: true,
            skills: true,
            web: true,
        }
    }

    fn names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        for (name, enabled) in [
            ("docs", self.docs),
            ("docker", self.docker),
            ("hooks", self.hooks),
            ("release", self.release),
            ("rust", self.rust),
            ("skills", self.skills),
            ("web", self.web),
        ] {
            if enabled {
                names.push(name);
            }
        }
        names
    }
}

#[derive(Debug)]
struct PlanStep {
    name: &'static str,
    command: &'static str,
}

pub fn run(root: &Path, args: PrePushArgs) -> Result<()> {
    let full = truthy(std::env::var("CORTEX_FULL_PRE_PUSH").ok().as_deref());
    let paths = if let Some(path) = args.changed_files {
        read_changed_files(&path)?
    } else {
        match resolve_base(root).and_then(|base| changed_files(root, &base)) {
            Ok(paths) => paths,
            Err(error) => {
                if !full {
                    eprintln!(
                        "pre-push: could not determine changed files ({error:#}); running fast \
                         minimal checks only. Set CORTEX_FULL_PRE_PUSH=1 for the full local suite."
                    );
                }
                Vec::new()
            }
        }
    };

    let categories = classify(&paths, full);
    let plan = command_plan(&paths, &categories, full);

    write_classifier_output(&paths, &categories);
    println!("Pre-push plan:");
    if plan.is_empty() {
        println!("  <none; CI remains the authoritative full validation gate>");
    } else {
        for step in &plan {
            println!("  {}: {}", step.name, step.command);
        }
    }
    if !full {
        println!(
            "Set CORTEX_FULL_PRE_PUSH=1 to run the full all-targets/all-features suite locally."
        );
    }

    if args.dry_run {
        return Ok(());
    }

    for step in plan {
        run_command(root, &step)?;
    }
    Ok(())
}

fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn resolve_base(root: &Path) -> Result<String> {
    if let Ok(base) = std::env::var("CORTEX_PRE_PUSH_BASE")
        && !base.trim().is_empty()
    {
        return Ok(base);
    }

    for candidate in ["@{upstream}", "origin/main"] {
        if !git_ref_exists(root, candidate) {
            continue;
        }
        if let Ok(base) = git_output(root, &["merge-base", candidate, "HEAD"]) {
            return Ok(base);
        }
    }

    git_output(root, &["rev-parse", "HEAD^"]).context("failed to resolve HEAD^ fallback")
}

fn git_ref_exists(root: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "--quiet", reference])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn changed_files(root: &Path, base: &str) -> Result<Vec<String>> {
    let raw = git_output(root, &["diff", "--name-only", base, "HEAD"])?;
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {args:?}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn read_changed_files(path: &Path) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read changed-files input {}", path.display()))?;
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn classify(paths: &[String], full: bool) -> Categories {
    if full {
        return Categories::all();
    }

    let path_refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
    classify_paths(&path_refs)
}

fn classify_paths(paths: &[&str]) -> Categories {
    let rust = any_path(paths, &["src/", "tests/", "xtask/", ".cargo/", "scripts/"])
        || any_file(
            paths,
            &[
                "Cargo.toml",
                "Cargo.lock",
                "build.rs",
                "rust-toolchain.toml",
            ],
        );
    let web = any_path(paths, &["web/"]);
    let docker = web
        || any_path(paths, &["config/"])
        || any_file(
            paths,
            &[
                "config/Dockerfile",
                "docker-compose.yml",
                "docker-compose.prod.yml",
            ],
        );
    let skills = any_path(paths, &["plugins/cortex/skills/", ".claude-plugin/"]);
    let release = rust
        || any_path(paths, &["release/"])
        || any_file(
            paths,
            &[
                "CHANGELOG.md",
                "server.json",
                "mcpb/manifest.json",
                "docker-compose.prod.yml",
            ],
        );
    let hooks = any_file(paths, &["lefthook.yml"])
        || any_path(paths, &["xtask/src/pre_push", "xtask/src/main"]);
    let docs = any_path(paths, &["docs/"]) || any_file(paths, &["README.md"]);

    Categories {
        docs,
        docker,
        hooks,
        release,
        rust,
        skills,
        web,
    }
}

fn command_plan(paths: &[String], categories: &Categories, full: bool) -> Vec<PlanStep> {
    let mut plan = Vec::new();

    if full || categories.release {
        plan.push(PlanStep {
            name: "version-sync",
            command: "cargo xtask check-version-sync",
        });
    }
    if full || categories.hooks {
        plan.push(PlanStep {
            name: "pre-push-router-tests",
            command: "cargo test -p xtask pre_push --locked",
        });
    }
    if full || categories.web {
        plan.push(PlanStep {
            name: "web-app-tests",
            command: "env -u CORTEX_API_TOKEN -u NO_AUTH cargo test web_app --lib --locked",
        });
    }
    if full || categories.skills {
        plan.push(PlanStep {
            name: "skills",
            command: "just validate-skills",
        });
    }
    if full || categories.rust {
        plan.push(PlanStep {
            name: "module-size",
            command: "bash scripts/check-rust-module-size.sh --limit 500",
        });
        plan.push(PlanStep {
            name: "clippy",
            command: "cargo clippy --all-targets --all-features --locked -- -D warnings",
        });
    }
    if full {
        plan.push(PlanStep {
            name: "release-versions",
            command: "cargo xtask check-release-versions",
        });
        plan.push(PlanStep {
            name: "full-tests",
            command: "env -u CORTEX_API_TOKEN -u NO_AUTH cargo test --all-targets --all-features --locked",
        });
    } else if paths.is_empty() {
        plan.push(PlanStep {
            name: "version-sync",
            command: "cargo xtask check-version-sync",
        });
    }

    dedupe_plan(plan)
}

fn dedupe_plan(plan: Vec<PlanStep>) -> Vec<PlanStep> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for step in plan {
        if seen.insert(step.name) {
            out.push(step);
        }
    }
    out
}

fn configure_shell_command(command: &mut Command, step: &str) {
    // Use a non-login shell so hook and CI callers keep their selected PATH,
    // Cargo binary, compiler wrapper, and target directory. A login shell
    // reloads the user profile and can silently replace those with soldr shims.
    command.arg("-c").arg(step);
}

fn run_command(root: &Path, step: &PlanStep) -> Result<()> {
    println!("\n==> {}\n{}", step.name, step.command);
    let mut command = Command::new("bash");
    configure_shell_command(&mut command, step.command);
    command.current_dir(root);
    for (key, _) in std::env::vars() {
        if key.starts_with("CARGO_PROFILE_") {
            command.env_remove(key);
        }
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run {}", step.name))?;
    if !status.success() {
        anyhow::bail!("{} failed with {status}", step.name);
    }
    Ok(())
}

fn write_classifier_output(paths: &[String], categories: &Categories) {
    println!("Changed files:");
    if paths.is_empty() {
        println!("  <none relative to selected base>");
    } else {
        for path in paths {
            println!("  {path}");
        }
    }
    let names = categories.names();
    if names.is_empty() {
        println!("Enabled categories: <none>");
    } else {
        println!("Enabled categories: {}", names.join(", "));
    }
}

fn starts(path: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|prefix| path == prefix.trim_end_matches('/') || path.starts_with(prefix))
}

fn any_path(paths: &[&str], prefixes: &[&str]) -> bool {
    paths.iter().any(|path| starts(path, prefixes))
}

fn any_file(paths: &[&str], names: &[&str]) -> bool {
    paths.iter().any(|path| names.contains(path))
}

#[cfg(test)]
#[path = "pre_push_tests.rs"]
mod tests;
