---
name: report
description: Use when writing a time-bounded Cortex log or fleet-health report with sourced findings.
---

# Report

Establish the requested time window, then check `stats`, `hosts`, and bounded `errors`/`search`. Correlate only specific spikes. Report exact timestamps, hosts, severity, representative messages, source actions, coverage gaps, and follow-up work. A missing host is a coverage gap, not proof of health.

## Workflow

Use the current Cortex `action=help` schema before unfamiliar parameters. For the full decision tree, output structure, and examples, read [the detailed workflow](references/workflow.md) when this task needs it. Cite the exact evidence and distinguish observations, likely causes, and gaps.
