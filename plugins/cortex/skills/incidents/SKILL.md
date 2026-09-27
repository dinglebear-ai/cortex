---
name: incidents
description: Use when triaging Cortex unaddressed error signatures, alert firings, similar incidents, or incident context.
---

# Incidents

Start with `unaddressed_errors` for the current backlog. Use `notifications_recent`, `similar_incidents`, or `incident_context` for the user’s concrete question. Acknowledge only a named, understood signature; `ack_error` and `unack_error` require admin scope and change server state.

## Workflow

Use the current Cortex `action=help` schema before unfamiliar parameters. For the full decision tree, output structure, and examples, read [the detailed workflow](references/workflow.md) when this task needs it. Cite the exact evidence and distinguish observations, likely causes, and gaps.
