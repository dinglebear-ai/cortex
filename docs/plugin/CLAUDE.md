<!--
SPDX-License-Identifier: MIT
Author: jmagar
License: MIT
Description: Plugin surface documentation index for the cortex Claude Code plugin.
-->

# Plugin Surface Documentation -- cortex

Index for the `plugin/` documentation subdirectory. These docs cover every Claude Code plugin surface area available to cortex.

## File index

| File | Purpose |
| --- | --- |
| `CHANNELS.md` | Channel integration (none) |
| `CONFIG.md` | Plugin settings: userConfig, settings.json |
| `HOOKS.md` | Plugin setup lifecycle — no Claude Code hooks; run `cortex setup pluginhook` manually |
| `MARKETPLACES.md` | Marketplace publishing: Claude, Codex, Gemini, MCP Registry |
| `OUTPUT-STYLES.md` | Output style definitions (none) |
| `PLUGINS.md` | Plugin manifest reference: .claude-plugin, .codex-plugin, gemini-extension |
| `SCHEDULES.md` | Scheduled tasks (none) |
| `SKILLS.md` | Skill definitions under `plugins/cortex/skills/`, including MCP usage, reports, diagnostics, deployment, logs, and version checks |

## Agent definitions

cortex does not define plugin-local agents. The MCP tools are consumed directly
by external agents (Claude Code, Codex, Gemini) without an intermediary agent
layer.

The consuming agents live outside this repo:

- `claude-homelab` homelab-core agents can call cortex tools for log analysis.
- Custom agents in other repos can connect to cortex via HTTP transport.

See [../mcp/TOOLS.md](../mcp/TOOLS.md) for the tool surface and
[../mcp/CONNECT.md](../mcp/CONNECT.md) for connection patterns.
