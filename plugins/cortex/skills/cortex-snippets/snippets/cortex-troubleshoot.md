---
name: cortex-troubleshoot
description: Check Cortex service and ingest status through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
tools:
  - cortex::cortex
---

# Check Cortex service and ingest status

Replacement for the Cortex `troubleshoot` skill's evidence collection. Use status and compose_doctor evidence to choose a targeted diagnostic. Do not infer root cause from a missing response alone.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const batch = await codemode.batch([
    () => callTool("cortex::cortex", { action: "status" }),
    () => callTool("cortex::cortex", { action: "compose_doctor" })
  ]);
  return { ok: batch.all_ok, snippet: "cortex-troubleshoot",
    results: batch.ok.map(({ i, value }) => ({ source: ["status", "compose_doctor"][i], preview: JSON.stringify(value).slice(0, 1800) })),
    failures: batch.failed.map(({ i, error }) => ({ source: ["status", "compose_doctor"][i], error: String(error).slice(0, 500) })),
    guidance: "Choose a targeted connection, ingest, or service test from observed failures." };
}
```
