---
name: cortex-report
description: Collect bounded fleet report evidence through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
inputs:
  since:
    type: string
    required: false
    default: 24h
tools:
  - cortex::cortex
---

# Collect bounded fleet report evidence

Replacement for the Cortex `report` skill's evidence collection. This snippet batches stats, hosts, and errors before writing a report. Preserve the exact window and distinguish missing coverage.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const since = input.since ?? "24h";
  const [stats, hosts, errors] = await codemode.batch([
    () => callTool("cortex::cortex", { action: "stats" }),
    () => callTool("cortex::cortex", { action: "hosts" }),
    () => callTool("cortex::cortex", { action: "errors", since })
  ]);
  return { ok: true, snippet: "cortex-report", window: { since }, stats, hosts, errors,
    guidance: "Use exact timestamps and source coverage; correlate only concrete error spikes." };
}
```
