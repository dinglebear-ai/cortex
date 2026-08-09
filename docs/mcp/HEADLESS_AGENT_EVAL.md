---
title: "Headless Agent Prompt Evaluation"
created: "2026-07-30"
updated: "2026-07-30"
---

# Headless Agent Prompt Evaluation

cortex already ingests Claude, Codex, and Gemini sessions, so prompt
evaluation should prefer live agent runs over a synthetic-only harness.

The practical shape is:

1. Render a prompt from the running MCP server with `prompts/get`.
2. Let a headless agent run the prompt against the same cortex MCP server it
   normally uses.
3. Ask the agent to return JSON conforming to `cortex://schema/prompt-output`.
4. Score the final shape and, when session ingestion is enabled, inspect the
   recorded session for action order, bounds, and evidence-before-claim
   behavior.

Use `scripts/prompt-headless-eval.sh` for the live path:

```bash
scripts/prompt-headless-eval.sh --dry-run --prompt infra.service-outage --arg service=plex
scripts/prompt-headless-eval.sh --agent codex --report /tmp/syslog-prompt-eval.json --prompt infra.after-deploy-check --arg service=cortex
scripts/prompt-headless-eval.sh --agent claude --mcp-config /path/to/claude-mcp-config.json --report /tmp/syslog-prompt-eval.json --prompt infra.storage-pressure
```

The script deliberately has two preflight layers:

- MCP server preflight always calls `prompts/get`, `resources/read`,
  `tools/list`, and `cortex action=help` over JSON-RPC. This proves the live
  server exposes prompts, the prompt output schema resource, the `cortex` tool,
  and action cost metadata. `mcporter` is still useful for tools, but current
  installed mcporter does not expose first-class MCP resource reads, so resource
  checks stay as raw JSON-RPC.
- Agent MCP preflight asks the selected headless agent to call `cortex
  action=help` using only its configured MCP tools. This fails fast when the
  agent runtime cannot see cortex directly. Use `--skip-agent-preflight`
  only when intentionally evaluating fallback behavior.

Cost controls:

- `--timeout SECS` bounds the full agent run; default is 300 seconds.
- `--preflight-timeout SECS` bounds the agent MCP preflight; default is 90
  seconds.
- `--max-tokens N` fails the run when parsed token usage exceeds the budget;
  default is 25000 and `0` disables the check. Codex token usage is parsed from
  its final `tokens used` line.
- `--max-budget-usd AMOUNT` is passed through to Claude `--print`.

Reports:

- `--report PATH` writes compact JSON with status, prompt, agent, MCP URL,
  preflight results, token count, verdict, confidence, evidence count, next
  action count, telemetry gap count, and the validated structured output.
- Full agent logs and raw/validated outputs are copied to `PATH.artifacts/`
  when a full agent run is attempted.

If the agent MCP preflight fails, fix the agent's MCP configuration first. In
the observed Codex environment, the headless run saw `lab`, `codex_apps`,
`context7`, and `winhost-windows-mcp`, but not `cortex`; the script now stops at
that point instead of spending a large token budget exploring local fallbacks.

## Claude Code Headless Surface

Live `claude --help` plus Axon-backed docs identify `claude -p` /
`claude --print` as the non-interactive mode.

Important headless flags:

- `-p`, `--print`: print a response and exit.
- `--output-format text|json|stream-json`: choose final or streaming output.
- `--input-format text|stream-json`: use text or streaming JSON input.
- `--json-schema <schema>`: validate structured output.
- `--include-partial-messages`: include partial chunks with `stream-json`.
- `--include-hook-events`: include hook lifecycle events with `stream-json`.
- `--no-session-persistence`: avoid writing session files.
- `--session-id <uuid>`, `--resume`, `--continue`, `--fork-session`: control
  session identity and resumption.
- `--bare`: skip hooks, LSP, plugin sync, attribution, auto-memory,
  background prefetches, keychain reads, and CLAUDE.md discovery. Useful for
  deterministic CI runs when all context is supplied explicitly.
- `--mcp-config <file-or-json>` and `--strict-mcp-config`: control the MCP
  surface available to the run.
- `--tools`, `--allowedTools`, `--disallowedTools`: bound tool access.
- `--permission-mode default|acceptEdits|auto|bypassPermissions|dontAsk|plan`:
  control approval behavior.
- `--system-prompt`, `--append-system-prompt`, `--settings`, `--plugin-dir`,
  `--plugin-url`, `--model`, `--effort`, `--max-budget-usd`: control runtime
  policy, plugins, model, and budget.

## Codex Headless Surface

Live `codex --help` and `codex exec --help` identify `codex exec` as the
non-interactive headless mode.

Important headless flags:

- `codex exec [PROMPT]`: run non-interactively; reads stdin when prompt is
  omitted or `-` is used.
- `--json`: print agent events to stdout as JSONL.
- `--output-schema <file>`: require the final response to match a JSON Schema.
- `--output-last-message <file>`: write the final assistant message to a file.
- `--ephemeral`: run without persisting session files.
- `--ignore-user-config`, `--ignore-rules`, `--strict-config`: make CI runs
  more deterministic.
- `--cd <dir>`, `--add-dir <dir>`: control workspace access.
- `--sandbox read-only|workspace-write|danger-full-access`: set command
  sandboxing.
- `--dangerously-bypass-approvals-and-sandbox`: use only inside an external
  sandbox.
- `--model`, `--profile`, `--profile-v2`, `--config key=value`, `--enable`,
  `--disable`: control model and configuration.
- `--image <file>`: attach images to the initial prompt.
- `codex review`: non-interactive code review mode, with `--uncommitted`,
  `--base`, `--commit`, and `--title`.

## Why This Beats a Pure Fixture Harness

Synthetic fixtures are still useful for CI shape checks, but they cannot prove
that agents use the live MCP surface efficiently. Headless Claude/Codex runs can
exercise the real server, real auth, real session ingestion, and real tool
schemas. The deterministic part should stay narrow: prompt rendering, output
schema validation, known action references, bounded-call guidance, and invalid
parameter regressions.
