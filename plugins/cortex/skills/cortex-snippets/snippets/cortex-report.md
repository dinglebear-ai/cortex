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
  const previewEvidence = (evidence) => {
    let previewTruncated = false;
    let remaining = 120;
    const preview = (value, depth = 0) => {
      if (remaining-- <= 0 || depth > 6) {
        previewTruncated = true;
        return "[omitted]";
      }
      if (typeof value === "string") {
        if (value.length > 250) previewTruncated = true;
        return value.slice(0, 250);
      }
      if (!value || typeof value !== "object") return value;
      if (Array.isArray(value)) {
        if (value.length > 3) previewTruncated = true;
        return value.slice(0, 3).map(item => preview(item, depth + 1));
      }
      const result = Object.create(null);
      let count = 0;
      for (const key in value) {
        if (!Object.prototype.hasOwnProperty.call(value, key)) continue;
        if (count++ === 20 || remaining <= 0) {
          previewTruncated = true;
          break;
        }
        const sensitive = /message|text|content|stdout|stderr|transcript|command|token|secret|authorization|metadata|password|credential|api[_-]?key|private[_-]?key/i.test(key);
        if (key.length > 80) previewTruncated = true;
        result[key.slice(0, 80)] = sensitive && typeof value[key] !== "boolean" ? "[omitted]" : preview(value[key], depth + 1);
      }
      return result;
    };
    const bounded = preview(evidence);
    const serialized = JSON.stringify(bounded);
    return { preview: serialized.slice(0, 1500), preview_truncated: previewTruncated || serialized.length > 1500 };
  };
  const since = input.since ?? "24h";
  const batch = await codemode.batch([
    () => callTool("cortex::cortex", { action: "stats" }),
    () => callTool("cortex::cortex", { action: "hosts" }),
    () => callTool("cortex::cortex", { action: "errors", since })
  ]);
  return { ok: batch.all_ok, snippet: "cortex-report", window: { since },
    results: batch.ok.map(({ i, value }) => ({ source: ["stats", "hosts", "errors"][i], ...previewEvidence(value) })),
    failures: batch.failed.map(({ i }) => ({ source: ["stats", "hosts", "errors"][i], error: "Upstream call failed; inspect the recorded tool failure." })),
    guidance: "Use exact timestamps and source coverage; correlate only concrete error spikes." };
}
```
