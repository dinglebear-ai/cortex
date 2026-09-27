# Repository scripts

Scripts support the binary-owned setup/runtime and repository validation. They are not a second deployment engine. The plugin ships no Claude Code lifecycle hooks.

## Validation

| Script | Responsibility |
| --- | --- |
| `scripts/check-agent-memory-symlinks.sh` | Require regular canonical `AGENTS.md` files and relative Claude/Gemini aliases |
| `scripts/test-agent-memory-symlinks.sh` | Isolated regression fixtures for the instruction-authority contract |
| `scripts/check-rust-module-size.sh` | Rust module-size policy and reviewed exceptions |
| `scripts/check-public-identity.sh` | Public naming/distribution identity invariants |
| `scripts/check-private-identifiers.sh` | Private identifier guard |
| `scripts/block-env-commits.sh` | Staged credential-pattern guard |
| `scripts/validate-marketplace.sh` | Plugin manifest/packaging constraints, including no lifecycle hooks |
| `scripts/check-agent-observatory-contracts.sh` | Agent Observatory contract checks |
| `scripts/ci/` | Path classification and CI helper logic |

Run Bash entrypoints with `bash scripts/<name>.sh`; individual Python helpers may be invoked by their wrapper. `lefthook.yml`, `Justfile`, and `xtask/src/pre_push.rs` determine which checks run for a change.

## Runtime and maintenance

`plugin-setup.sh` maps plugin options and delegates to `cortex setup pluginhook`. `prepare-compose-dirs.sh` prepares Compose bind directories. `check-runtime-current.sh` compares runtime/container identity. Deployment/template helpers operate only on their explicit configuration and targets.

`backup.sh` uses SQLite online backup; `restore-backup.sh` and `reset-db.sh` affect persistent state and require coordinated writers and explicit operator intent. Read each helper's options and the applicable runbook first. Never run a reset to test a documentation change.

## Tests and packaging

`scripts/smoke-test.sh` and `tests/test_live.sh` are compatibility entrypoints for the canonical `tests/live/run-profile.sh` runner. The profiles own isolated services and evidence ledgers; production fleet/provider checks require separate grants. Numerous `scripts/test-*` fixtures validate backup, restore, environment, provisioning, and packaging behavior without treating a shell success marker as live fleet evidence.

`scripts/build-mcpb.sh` creates target-specific bundles and validates prerequisites. Release and container scripts remain governed by the release workflows; a local script run does not prove publication.

Bash scripts should use `#!/usr/bin/env bash`, `set -euo pipefail`, quoted paths, bounded operations, and nonzero failure exits. Keep temporary cleanup scoped to a directory created by the test itself.

See [RECIPES.md](RECIPES.md), [DOCUMENTATION.md](DOCUMENTATION.md), and [LIVE_QUALIFICATION.md](../LIVE_QUALIFICATION.md).
