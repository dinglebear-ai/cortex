---
name: cortex-incidents
description: Review unaddressed Cortex error signatures through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, triage]
inputs:
  limit:
    type: integer
    required: false
    default: 20
tools:
  - cortex::cortex
---

# Review unaddressed Cortex error signatures

Replacement for the Cortex `incidents` skill's evidence collection. Prioritize recent active signatures. Acknowledgement changes server state and requires a separate authorized action.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "unaddressed_errors", limit: input.limit ?? 20 };
  const evidence = await callTool("cortex::cortex", params);
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
      const sensitive = /message|text|content|stdout|stderr|transcript|command|arguments?|input|output|token|secret|authorization|metadata|password|credential|api[_-]?key|private[_-]?key/i.test(key);
      if (key.length > 80) previewTruncated = true;
      result[key.slice(0, 80)] = sensitive && typeof value[key] !== "boolean" ? "[omitted]" : preview(value[key], depth + 1);
    }
    return result;
  };
  const bounded = preview(evidence);
  const serialized = JSON.stringify(bounded);
  return { ok: true, snippet: "cortex-incidents", request: params, total_incidents: evidence.total_incidents, no_data: evidence.no_data, truncated: evidence.truncated,
    evidence_keys: Object.keys(bounded), evidence_preview: serialized.slice(0, 4000),
    preview_truncated: previewTruncated || serialized.length > 4000, guidance: 'Prioritize recent active signatures. Acknowledgement changes server state and requires a separate authorized action.' };
}
```
