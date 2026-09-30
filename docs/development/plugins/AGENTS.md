<!--
SPDX-License-Identifier: MIT
Author: jmagar
License: MIT
Description: Plugin surface documentation index for the cortex Claude Code plugin.
-->

# Plugin documentation

Follow the root [AGENTS.md](../../../AGENTS.md), [installer package instructions](../../../plugins/install-cortex/AGENTS.md), and [usage package instructions](../../../plugins/cortex/AGENTS.md). The generated [section index](README.md) lists every plugin guide.

The installer package owns `install-cortex` and the MCP client configuration. The usage package owns `using-cortex`, `cortex-snippets`, and eleven explicitly installed Labby snippet workflows. Retired standalone skills are not shipped merely because their workflows still exist as snippets. Runtime assessment prompts remain independently embedded from `src/prompts/`.

No plugin-local agents or automatic lifecycle hooks ship. Setup scripts delegate to the binary. Keep credentials and server/client roles distinct; validate both packages with `just validate-plugin`. Refer to [MCP tools](../../reference/mcp/tools.md) and [client connections](../../reference/mcp/connect.md) for the server surface.
