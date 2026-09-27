---
name: cortex-version-check
description: Check Cortex Compose state through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
tools:
  - cortex::cortex
---

# Check Cortex Compose state

Replacement for the Cortex `version-check` skill's evidence collection. Compose status is not an image identity comparison. Use the bundled check-runtime-current.sh on the service host to prove image freshness.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "compose_status" };
  const evidence = await callTool("cortex::cortex", params);
  return { ok: true, snippet: "cortex-version-check", request: params, evidence, guidance: 'Compose status is not an image identity comparison. Use the bundled check-runtime-current.sh on the service host to prove image freshness.' };
}
```
