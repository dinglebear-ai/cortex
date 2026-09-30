# Mandatory compatibility track: transcript-forwarding environment variable rename

Status: required before Agent Observatory provider and deployment rollout
Tasks: ENV-001 through ENV-004
Target replacement: `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`
Deprecated alias: `CORTEX_AGENT_AI_TRANSCRIPTS`

## Why this track exists

The legacy name sounds like the master switch for AI transcript ingestion. It is not. It controls only whether `cortex heartbeat agent` forwards locally discovered transcript records to a remote Cortex HTTP endpoint. The independent local `cortex sessions watch` service continues to ingest directly into its configured SQLite database.

The replacement name must make the network-forwarding behavior unmistakable and align with sibling variables such as `CORTEX_AGENT_COMMAND_FORWARD` and `CORTEX_AGENT_SHELL_HISTORY_FORWARD`.

## Frozen compatibility behavior

Resolution order:

1. When `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD` is set, it is authoritative.
2. When both names are set to equivalent values, the new name wins and Cortex emits one legacy-alias warning.
3. When both names conflict, the new name wins and Cortex emits one conflict warning naming both variables.
4. When only `CORTEX_AGENT_AI_TRANSCRIPTS` is set, Cortex honors it and emits one deprecation warning.
5. When neither name is set, remote transcript forwarding remains disabled.

Warnings must never claim that local transcript watching or local database ingestion is disabled. Generated configuration must emit only the new name. The deprecated alias remains accepted throughout Cortex 3.x and is scheduled for removal in Cortex 4.0 after a changelog entry and release gate.

## ENV-001 Add the new resolver and compatibility alias

- **Deliverable:** heartbeat-agent configuration reads `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD` with the frozen precedence and warning behavior above.
- **Files:** `src/heartbeat_agent.rs`, its sidecar tests, and any shared boolean-env helper extracted by the implementation.
- **RED:** table-driven tests fail for neither/new-only/legacy-only/both-equal/both-conflicting inputs and assert the resulting forwarding boolean plus warning code.
- **GREEN:** add named constants and one resolver; keep the internal field semantics explicitly named as transcript forwarding rather than generic transcript enablement where practical.
- **Proof:** focused tests pass without reading the real process environment; one integration test proves `sessions watch` behavior is unchanged when both forwarding variables are false.
- **Gate:** the old variable is read only by the compatibility resolver and cannot become authoritative when the new variable is present.
- **References:** `src/heartbeat_agent.rs`, contract §10.1, specification §14.

## ENV-002 Switch all generated and deployed configuration to the new name

- **Deliverable:** setup, deploy, allowlist, generated env files, and tests emit and preserve `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD` only.
- **Files:** `src/setup/heartbeat_agent.rs`, `src/setup/heartbeat_agent_tests.rs`, `src/agent_deploy.rs`, `src/agent_deploy_tests.rs`, and any checked-in service/env templates discovered by the implementation.
- **RED:** generated-env and deployment fixtures still contain `CORTEX_AGENT_AI_TRANSCRIPTS` or omit the new name.
- **GREEN:** update generators, copy/merge logic, environment allowlists, and assertions while preserving the configured boolean value.
- **Proof:** generation/deployment tests pass; a repository grep finds the legacy name only in the compatibility resolver, compatibility tests, migration docs, and changelog.
- **Gate:** no generated production configuration contains both names.
- **References:** current setup and agent-deploy tests listed above.

## ENV-003 Add doctor detection and safe automatic migration

- **Deliverable:** doctor reports legacy-only, both-equal, and conflicting configurations and safely migrates unambiguous files under explicit `--fix --yes`.
- **Files:** `src/setup/doctor.rs`, setup resolution helpers, sidecar tests, and operator output models.
- **RED:** temporary env-file fixtures show no diagnostic or unsafe rewriting.
- **GREEN:** implement these exact cases: legacy-only atomically renames the key; both-equal removes the legacy line; conflicting values produce an error and no write; missing files remain unchanged.
- **Proof:** fixture tests verify contents, permissions, atomic replacement behavior, warning/error code, and idempotent second run.
- **Gate:** no automatic migration occurs without explicit fix authorization; conflict resolution is never guessed.
- **References:** existing doctor fix authorization and private-file write patterns.

## ENV-004 Complete documentation, release, and removal gates

- **Deliverable:** all current docs, examples, deployment templates, help, status, and release notes describe the new variable as remote transcript forwarding and identify the old name only as a deprecated alias.
- **Files:** root README, `docs/CONFIG.md`, `docs/INVENTORY.md`, config/env contracts, deployment examples, changelog, generated help, and release verification scripts.
- **RED:** a compatibility grep allows the old name in production templates or documentation presents it as the current setting.
- **GREEN:** add a strict occurrence allowlist and a Cortex 4.0 removal checklist; status/doctor surfaces identify legacy use without exposing values.
- **Proof:** docs/generated checks pass; new installations contain only `CORTEX_AGENT_AI_TRANSCRIPT_FORWARD`; an upgrade fixture using only the old name still forwards and emits the expected warning.
- **Gate:** removal before Cortex 4.0 fails the compatibility test; Cortex 4.0 removal must delete the alias, warning, migration code, and allowlist entry together.
- **References:** execution contract G7/G8 and the compatibility sections of the spec/contract.

## Track gate

This track is green only when:

- the new name is authoritative
- legacy-only configurations still work with one warning
- conflicting dual configuration is deterministic and visible
- generated configuration uses only the new name
- doctor safely rewrites only unambiguous files
- local `sessions watch` ingestion is proven independent
- the old name is restricted to compatibility code/tests/migration documentation
- the Cortex 4.0 removal gate is documented and tested
