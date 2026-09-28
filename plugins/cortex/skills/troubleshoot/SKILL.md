---
name: troubleshoot
description: Use when diagnosing Cortex MCP connection failures, missing logs, unhealthy containers, or restart loops.
---

# Troubleshoot

Choose the branch that matches the failure: client connection/auth, sender ingest, server health, or uncertain symptoms. Check a cheap observation first with `status`, `hosts`, or `compose_doctor`; then test one diagnostic hypothesis. Do not restart services, kill processes, or change sender configuration merely to inspect the problem.

## Workflow

Use the current Cortex `action=help` schema before unfamiliar parameters. For the full decision tree, output structure, and examples, read [the detailed workflow](references/workflow.md) when this task needs it. Cite the exact evidence and distinguish observations, likely causes, and gaps.
