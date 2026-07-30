---
title: "Plugin Setup -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

<!--
SPDX-License-Identifier: MIT
Author: jmagar
License: MIT
Description: Plugin setup lifecycle for the cortex plugin (no automatic Claude Code hooks).
-->

# Plugin Setup -- cortex

The cortex plugin ships **no Claude Code lifecycle hooks**. There is no
`plugins/cortex/hooks/hooks.json`, and `.claude-plugin/plugin.json` declares no
`hooks` key. Nothing runs automatically on `SessionStart` or `ConfigChange`.

Setup is explicit and operator-driven: run `cortex setup` yourself after you
install, upgrade, or reconfigure the plugin.

## File location

```
plugins/
  cortex/
    scripts/
      plugin-setup.sh       # Thin adapter: CLAUDE_PLUGIN_OPTION_* -> env -> cortex setup pluginhook
      check-runtime-current.sh
      smoke-test.sh
scripts/
  plugin-setup.sh           # Repo-root copy of the same adapter
```

## Setup is owned by the binary

`cortex setup pluginhook` is the single entrypoint. It:

- Server mode: exports the current Claude Code `userConfig` values as
  `CORTEX_*` environment variables.
- Ensures a `cortex` binary exists on `PATH`; if it is missing, runs the
  one-line installer.
- Delegates host setup to `cortex setup repair`, which owns `~/.cortex/.env`,
  `~/.cortex/compose/`, and the Docker Compose container.
- Client mode: skips local server setup and only checks the configured
  server's `/health` endpoint.

| Command | Behavior |
| --- | --- |
| `cortex setup check` | Read-only; reports drift, changes nothing |
| `cortex setup repair` | Idempotent; converges the host to the desired state |
| `cortex setup pluginhook` | Maps plugin options, then repairs |
| `cortex setup pluginhook --no-repair` | Audit mode; maps options and checks only |

## Manual execution

Run the binary-owned setup directly:

```bash
CLAUDE_PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$PWD/plugins/cortex}" \
  "$CLAUDE_PLUGIN_ROOT/bin/cortex" setup pluginhook
```

Or go through the adapter, which maps `CLAUDE_PLUGIN_OPTION_*` values to
environment variables before delegating to the binary:

```bash
bash scripts/plugin-setup.sh
```

Setup deliberately contains no separate Compose rendering logic. The Claude
plugin and the one-line installer both converge on the same `cortex setup`
implementation and the same `~/.cortex` host layout.

## See also

- [../GUARDRAILS.md](../GUARDRAILS.md) -- security patterns
- [../mcp/PRE-COMMIT.md](../mcp/PRE-COMMIT.md) -- lefthook git hooks (unrelated to Claude Code hooks)
