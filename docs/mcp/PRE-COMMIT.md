---
title: "Pre-commit Hook Configuration -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# Pre-commit Hook Configuration -- cortex

Pre-commit checks run through [lefthook](https://github.com/evilmartians/lefthook),
configured in `lefthook.yml` at the repo root. cortex ships **no Claude Code
lifecycle hooks** — see [../plugin/HOOKS.md](../plugin/HOOKS.md).

## Hook configuration

Install the git hooks once per clone:

```bash
lefthook install
```

`lefthook.yml` pre-commit jobs (most wrapped in `scripts/with_timeout.sh` — the
`diff check` and `skills` jobs run unwrapped):

| Job | Command | Purpose |
| --- | --- | --- |
| diff check | `git --no-pager diff --check --cached` | Rejects whitespace errors and conflict markers in staged content |
| yaml parse | `python -c 'yaml.safe_load(...)'` | Rejects unparseable YAML in staged files |
| rustfmt | `cargo fmt -- --check` | Formatting gate |
| module size | `scripts/check-rust-module-size.sh --limit 500` | Caps Rust module line count |
| version sync | `cargo xtask check-version-sync` | All version-bearing files agree |
| skills | `just validate-skills` | Validates plugin/skill manifests and frontmatter (staged `plugins/cortex/skills/**`, `.claude-plugin/**`) |
| env guard | `scripts/block-env-commits.sh` | Blocks commits containing env credential patterns |


## Manual checks

Run the same gates by hand:

```bash
# Plugin manifest + skill frontmatter validation
just validate-plugin

# Marketplace manifest assertions
bash scripts/validate-marketplace.sh

# Version-bearing files agree
cargo xtask check-version-sync

# Env credential scan
bash scripts/block-env-commits.sh

# Dependency policy
cargo deny check
```

## Rust-specific checks

Before committing, run:

```bash
just fmt         # cargo fmt
just lint        # cargo clippy -- -D warnings
just test        # hermetic suite via cargo-nextest
```

`cargo xtask pre-push` bundles the heavier pre-push gate. All of these are also enforced in CI.

## See also

- [CICD.md](CICD.md) -- CI workflow enforces lint and test
- [../plugin/HOOKS.md](../plugin/HOOKS.md) -- plugin setup lifecycle (no Claude Code hooks)
- [../GUARDRAILS.md](../GUARDRAILS.md) -- security patterns
