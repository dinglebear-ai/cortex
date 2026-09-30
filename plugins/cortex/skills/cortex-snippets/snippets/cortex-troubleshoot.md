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
    return { preview: serialized.slice(0, 1800), preview_truncated: previewTruncated || serialized.length > 1800 };
  };
  const batch = await codemode.batch([
    () => callTool("cortex::cortex", { action: "status" }),
    () => callTool("cortex::cortex", { action: "compose_doctor" })
  ]);
  return { ok: batch.all_ok, snippet: "cortex-troubleshoot",
    results: batch.ok.map(({ i, value }) => ({ source: ["status", "compose_doctor"][i], ...previewEvidence(value) })),
    failures: batch.failed.map(({ i }) => ({ source: ["status", "compose_doctor"][i], error: "Upstream call failed; inspect the recorded tool failure." })),
    guidance: "Choose a targeted connection, ingest, or service test from observed failures." };
}
```
