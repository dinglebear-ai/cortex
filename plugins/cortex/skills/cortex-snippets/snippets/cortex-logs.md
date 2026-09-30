---
name: cortex-logs
description: Check Cortex service diagnostics through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
tools:
  - cortex::cortex
---

# Check Cortex service diagnostics

Replacement for the Cortex `logs` skill's evidence collection. This action gives Compose diagnostics, not stdout/stderr. To follow service logs, use cortex compose logs or Docker locally.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "compose_doctor" };
  const evidence = await callTool("cortex::cortex", params);
  return { ok: true, snippet: "cortex-logs", request: params, evidence, guidance: 'This action gives Compose diagnostics, not stdout/stderr. To follow service logs, use cortex compose logs or Docker locally.' };
}
```
