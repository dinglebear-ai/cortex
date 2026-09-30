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
  const batch = await codemode.batch([
    () => callTool("cortex::cortex", { action: "stats" }),
    () => callTool("cortex::cortex", { action: "hosts" }),
    () => callTool("cortex::cortex", { action: "errors", since })
  ]);
  return { ok: batch.all_ok, snippet: "cortex-report", window: { since },
    results: batch.ok.map(({ i, value }) => ({ source: ["stats", "hosts", "errors"][i], preview: JSON.stringify(value).slice(0, 1500) })),
    failures: batch.failed.map(({ i, error }) => ({ source: ["stats", "hosts", "errors"][i], error: String(error).slice(0, 500) })),
    guidance: "Use exact timestamps and source coverage; correlate only concrete error spikes." };
}
```
