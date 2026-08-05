# Session: Claude Code and Codex OTel Client Config

Date: 2026-05-08
Repo: `/home/jmagar/workspace/syslog-mcp`
Host context: local dev host with syslog MCP receiver reachable at `198.51.100.1:3100`

## Summary

Configured machine-local Claude Code and Codex telemetry clients to send OTLP logs to the syslog-mcp OTLP/HTTP receiver.

The receiver endpoint was verified from code before finalizing the client config:

- `src/otlp.rs` mounts OTLP/HTTP routes at `/v1/logs`, `/v1/metrics`, and `/v1/traces`.
- `src/main.rs` mounts that OTLP router on the same HTTP server as MCP.
- The correct client endpoints are therefore:
  - Claude Code base OTLP endpoint: `http://198.51.100.1:3100`
  - Codex log exporter endpoint: `http://198.51.100.1:3100/v1/logs`

## Files Changed

Machine-local files changed:

- `/home/jmagar/.claude/settings.json`
- `/home/jmagar/.codex/config.toml`

Repo artifact added:

- `docs/sessions/2026-05-08-claude-codex-otel-client-config.md`

No tracked source code changes were required for the OTel client setup.

## Claude Code Config

Merged OTel environment variables into `/home/jmagar/.claude/settings.json`.

Effective redacted values verified with `jq`:

```json
{
  "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
  "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
  "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
  "OTEL_EXPORTER_OTLP_ENDPOINT": "http://198.51.100.1:3100",
  "OTEL_EXPORTER_OTLP_HEADERS": "<set>",
  "OTEL_LOGS_EXPORTER": "otlp",
  "OTEL_METRICS_EXPORTER": "otlp",
  "OTEL_LOG_USER_PROMPTS": "1",
  "OTEL_LOG_TOOL_DETAILS": "1"
}
```

The bearer header was sourced from the existing syslog plugin environment file:

- `/home/jmagar/.claude/plugins/data/syslog-jmagar-lab/syslog-mcp.env`

The token value was not printed in chat or saved in this note.

## Codex Config

Added `[otel]` to `/home/jmagar/.codex/config.toml`.

Effective redacted values verified with Python `tomllib`:

```toml
[otel]
environment = "homelab"
log_user_prompt = true
metrics_exporter = "none"
trace_exporter = "none"
exporter = { otlp-http = { endpoint = "http://198.51.100.1:3100/v1/logs", protocol = "binary", headers = { Authorization = "Bearer <redacted>" } } }
```

The Codex OTLP header syntax was checked against the official Codex config reference:

- https://developers.openai.com/codex/config-reference

## Verification

Receiver health was checked with:

```bash
curl -sS --max-time 3 http://198.51.100.1:3100/health | jq '{status, otlp_logs_received, otlp_decode_errors}'
```

Latest observed result:

```json
{
  "status": "ok",
  "otlp_logs_received": 4169,
  "otlp_decode_errors": 0
}
```

OTLP auth and route behavior were verified directly:

- `POST http://198.51.100.1:3100/v1/logs` without auth returned `401`.
- `POST http://198.51.100.1:3100/v1/logs` with the configured bearer header returned `200`.

Claude Code ingest was confirmed in SQLite:

```text
2026-05-08T22:53:23.494Z || claude-code | info | claude_code.hook_execution_complete
2026-05-08T22:53:23.494Z || claude-code | info | claude_code.hook_execution_start
2026-05-08T22:53:23.492Z || claude-code | info | claude_code.api_request
```

Codex-related rows were observed after setup, but they were tagged as `app_name=node` rather than `app_name=codex`:

```text
2026-05-08T22:37:56.300Z | devhost | node | info | Embedded agent failed before reply: No API key found for provider "openai"...
2026-05-08T22:37:56.299Z | devhost | node | info | [model-fallback/decision] model fallback decision...
2026-05-08T22:37:56.299Z | devhost | node | info | [diagnostic] lane task error...
```

`codex debug models` also loaded the updated config successfully.

## Important Caveats

- syslog-mcp currently ingests OTLP logs.
- `/v1/metrics` returns `200` but discards metrics.
- `/v1/traces` returns `404`, so traces are not stored by this receiver yet.
- Claude Code had `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1` enabled as requested, but trace export will not persist here until syslog-mcp implements `/v1/traces`.
- Codex rows were not cleanly tagged as `codex` in the observed DB samples. They appeared as `app_name=node` with Codex/OpenClaw-related message text.
- Long-running Codex processes that started before the config change may need to be restarted for full ongoing interactive telemetry.

## Current Repo State

Before adding this note, `git status --short` was clean.

The only intended repo change from this save operation is this markdown session artifact.

## Open Questions

- Should syslog-mcp enrich Codex OTLP records so `app_name` becomes `codex` instead of `node` when the resource/process metadata indicates Codex?
- Should `/v1/traces` be implemented, or should trace data be routed to a real tracing backend such as Tempo or Jaeger instead?
- Should the Codex/Claude OTel bearer token be kept inline in config, or moved to a supported environment-variable indirection if Codex supports one for exporter headers?
