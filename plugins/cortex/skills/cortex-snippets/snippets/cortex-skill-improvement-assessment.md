---
name: cortex-skill-improvement-assessment
description: Investigate skill usage problems through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
inputs:
  skill:
    type: string
    required: true
tools:
  - cortex::cortex
---

# Investigate skill usage problems

Replacement for the Cortex `skill-improvement-assessment` skill's evidence collection. Use the evidence to assess triggering, instructions and observed agent behavior. Distinguish skill defects from tool failures or model noncompliance.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "skill_investigate", skill: input.skill };
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
      const sensitive = /message|text|content|stdout|stderr|transcript|command|token|secret|authorization|metadata|password|credential|api[_-]?key|private[_-]?key/i.test(key);
      if (key.length > 80) previewTruncated = true;
      result[key.slice(0, 80)] = sensitive && typeof value[key] !== "boolean" ? "[omitted]" : preview(value[key], depth + 1);
    }
    return result;
  };
  const bounded = preview(evidence);
  const serialized = JSON.stringify(bounded);
  return { ok: true, snippet: "cortex-skill-improvement-assessment", request: params,
    total_incidents: evidence.total_incidents, no_data: evidence.no_data,
    truncated: evidence.truncated, no_incident_low_severity_summary: evidence.no_incident_low_severity_summary,
    evidence_keys: Object.keys(bounded), evidence_preview: serialized.slice(0, 4000),
    preview_truncated: previewTruncated || serialized.length > 4000,
    guidance: 'This is a redacted, partial preview. Fetch the incident directly before assessing triggering, instructions and observed agent behavior. Distinguish skill defects from tool failures or model noncompliance.' };
}
```
