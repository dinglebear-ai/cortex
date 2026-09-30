# Cortex documentation

Start with the [project overview](../README.md) and [setup guide](guides/setup.md). Contributors should read [AGENTS.md](../AGENTS.md) and [CONTRIBUTING.md](../CONTRIBUTING.md).

## Browse by task

| Section | What belongs here |
| --- | --- |
| [Operator guides](guides/README.md) | Installation, OAuth, security, privacy, and operational runbooks |
| [Reference](reference/README.md) | CLI, REST, MCP, configuration, source inventory, and contracts |
| [Development](development/README.md) | Rust setup, source extensions, tests, release, plugins, and repository maintenance |
| [Architecture](architecture/README.md) | Current architecture, decisions, designs, and feature specifications |
| [History](history/README.md) | Dated plans, investigations, reports, reviews, and session evidence |

## Common starting points

[Setup](guides/setup.md) · [Configuration](reference/config.md) · [CLI](reference/cli.md) · [REST API](reference/api.md) · [MCP tools](reference/mcp/tools.md) · [Architecture](architecture/overview.md) · [Live qualification](development/live-qualification.md) · [Release checklist](development/release.md)

The [host metrics producer](../deploy/otel/hostmetrics/README.md) is a separate Collector deployment. Its assets stay with their deployment owner, not in a duplicate documentation tree.

## Current behavior versus design and history

Current operator guidance and executable source define implemented behavior. Contracts state their own status; a design, specification, or task checklist does not prove delivery. In particular, the [agent protocol](reference/contracts/agent-protocol.md) records a WebSocket design, while current host agents use implemented HTTP ingestion endpoints.

The history section preserves prior evidence and code examples at their recorded dates. Session notes and large planning collections are grouped by month. Their links remain navigable, but old deployment commands, versions, and source paths are not instructions for a current installation. Nothing was discarded during the documentation reorganization.

## Maintaining this tree

Ordinary filenames use lowercase kebab-case. `README.md` indexes and the canonical `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` instruction filenames retain their discovery conventions. All section indexes are generated from the document tree and titles; this home page is maintained by hand.

Run `just docs-generate` after changing titles, adding or moving documents, or changing generated inputs. Run `just docs-check` to verify generation, filenames, local links, anchors, and generator regression tests without modifying tracked files. See the [documentation maintenance contract](development/repo/documentation.md) for ownership and validation details.
