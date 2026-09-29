# Cortex documentation

Start with the [project overview](../README.md), canonical [AGENTS.md](../AGENTS.md), and [contributor guide](../CONTRIBUTING.md). Cortex covers logs, telemetry, AI sessions, host inventory, and evidence-backed investigation; it is no longer only a syslog MCP server.

## Current guides

| Document | Purpose |
| --- | --- |
| [SETUP.md](SETUP.md) | Install, configure, deploy, and verify |
| [CONFIG.md](CONFIG.md) | Runtime configuration and storage controls |
| [CLI.md](CLI.md) | CLI commands and routing |
| [api.md](api.md) | REST endpoint contracts, authorization, response bounds |
| [architecture.md](architecture.md) | Source families, services, storage, projections, ownership |
| [ADDING_SOURCES.md](ADDING_SOURCES.md) | Extend source adapters without duplicating registries or confusing transport with provider |
| [SECURITY.md](SECURITY.md), [OAUTH.md](OAUTH.md) | Trust model and OAuth/operator configuration |
| [GUARDRAILS.md](GUARDRAILS.md), [REDACTION.md](REDACTION.md) | Safety and privacy handling |
| [INVENTORY.md](INVENTORY.md) | Component and public surface inventory |
| [RUST.md](RUST.md) | Pinned toolchain, workspace, SDK, build/test setup |
| [RELEASE.md](RELEASE.md), [RELEASING.md](../RELEASING.md) | Release-please policy and release gates |
| [LIVE_QUALIFICATION.md](LIVE_QUALIFICATION.md) | Isolated live profiles versus explicitly granted fleet checks |
| [Tootie host metrics producer](../deploy/otel/hostmetrics/README.md) | Separate OpenTelemetry Collector that sends real host metrics to Cortex |
| [CHECKLIST.md](CHECKLIST.md) | Supplemental pre-release review |
| [repo/DOCUMENTATION.md](repo/DOCUMENTATION.md) | Documentation authority, maintenance, validation |

## Areas

| Directory | Scope |
| --- | --- |
| [mcp/](mcp/AGENTS.md) | MCP transport, auth, tools/resources, testing, distribution |
| [plugin/](plugin/AGENTS.md) | Plugin packaging, setup adapters, skills and supported surfaces |
| [repo/](repo/AGENTS.md) | Repository layout, Git, scripts, recipes, project knowledge |
| [stack/](stack/AGENTS.md) | Technology and prerequisites |
| [upstream/](upstream/AGENTS.md) | Source families and optional outbound integrations |
| `contracts/` | Behavior/storage/API contracts and their implementation status |
| `specs/`, `design/` | Feature specifications and design rationale; inspect status before claiming implementation |
| `runbooks/` | Operational procedures; current unless explicitly marked historical |

## Agent Observatory

Current implementation areas include `src/agent_observatory*`, `src/app/agent_observatory*`, `src/db/agent_observatory*`, `src/git_observer*`, `src/web_app.rs`, and `web/`. Begin with [architecture.md](architecture.md) and the [agent protocol contract](contracts/agent-protocol.md).

The original [research ledger](research/2026-07-31-agent-observatory.md), [architecture design](design/agent-observatory-architecture.md), [UI design](design/agent-observatory-ui.md), [specification](specs/agent-observatory.md), [contract](contracts/agent-observatory.md), and [implementation plan](plans/2026-07-31-agent-observatory-implementation.md) preserve proposal history. Their original task lists do not prove that all planned capabilities shipped, nor should the whole feature still be described as only a proposal when source exists.

## Historical material

Dated files in `plans/`, `research/`, `reports/`, `reviews/`, `sessions/`, and `superpowers/` describe their recorded point in time. `rollout.md` is the historical v0.26 HTTP-CLI migration playbook, not the default installation guide. Prefer current guides and executable contracts for command names, paths, auth, defaults, and version policy.

Use `AGENTS.md` as the cross-agent instruction target; its `CLAUDE.md` and `GEMINI.md` aliases are symlinks. Keep [CHANGELOG.md](../CHANGELOG.md) as version history, not a replacement for current operator documentation.
