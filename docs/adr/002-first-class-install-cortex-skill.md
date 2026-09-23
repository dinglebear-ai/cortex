---
title: "ADR 002: First-Class Cortex Installer Skill"
created: 2026-09-18
updated: 2026-09-18
---

# ADR 002: First-Class Cortex Installer Skill

**Status:** Accepted
**Date:** 2026-09-18

## Context

Cortex already has a canonical native installer, idempotent setup repair, server/client plugin configuration, Google OAuth, separate MCP/REST credentials, Compose lifecycle, storage guardrails, and Codex app-server assessments. What is missing is one first-run workflow that makes the fleet topology and security choices explicit before configuration.

A Cortex fleet has one authoritative ingest/storage server. Other machines usually act as MCP clients or host-local forwarders. Syslog itself is unauthenticated, while HTTP services have distinct credentials and scopes. OAuth also defaults to disabling the static MCP bearer unless the operator deliberately retains it.

## Decision

Add `install-cortex` to the existing Cortex plugin. The skill downloads the canonical `install.sh` as a reviewable file, delegates setup to `cortex setup repair`, and asks the user to choose server or client-only role before any listener/storage work.

Server setup configures syslog sender network controls, HTTP/MCP/REST credentials, storage policy, optional Google OAuth, and Compose persistence. Client-only setup never starts another receiver/database. OAuth plus static bearer is an explicit break-glass choice, not an implicit default.

Remote exposure uses current proxy/Tailscale documentation and a backup/diff/approval gate. Optional local assessment uses Cortex's existing `CORTEX_LLM=codex` Codex app-server backend.

## Consequences

- First-run setup reflects Cortex's actual fleet topology instead of treating every machine as a server.
- The skill remains thin over executable setup/Compose contracts.
- Syslog network trust and HTTP authentication cannot be accidentally conflated.
- Native installer/setup changes remain testable independently from agent guidance.
