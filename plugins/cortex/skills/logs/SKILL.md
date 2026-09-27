---
name: logs
description: Use when tailing or following cortex service logs from Docker Compose. Use when the user asks for cortex service logs, startup logs, crash logs, plugin deployment logs, Docker logs, or follow mode. This is for the service's stdout/stderr, not client syslog entries.
---

# Cortex Service Logs

Show recent cortex service logs. These are the binary's stdout/stderr, not syslog entries received from clients. Use the `cortex` MCP tool with `action=tail` for received log entries.

## Workflow

1. Check server mode:

   ```bash
   echo "$CLAUDE_PLUGIN_OPTION_IS_SERVER"
   ```

   If `false`, explain that the local plugin is in client mode and has no local service logs to tail.

2. Parse user arguments:
   - empty: last 50 lines, no follow
   - bare integer such as `100`: that many lines, no follow
   - `--follow` or `-f`: last 50 lines, then stream
   - integer plus follow flag: that many lines, then stream

3. Run consolidated Cortex Compose logs:

   ```bash
   cortex compose logs cortex --tail <N>
   ```

   For follow mode, use Docker Compose directly until Cortex grows a follow
   flag on the consolidated command.

   If the plugin compose project has no container, report that Docker mode is
   configured but no plugin-managed container is running. Do not guess a source
   checkout path.

## Output

Show the captured logs directly. If follow mode is active, the streaming output is the deliverable until the user interrupts.

If the logs show obvious unhealthy patterns such as repeated restarts, panics, bind errors, or DB lock errors, summarize the pattern after the log excerpt and use `troubleshoot` for full diagnostics.
