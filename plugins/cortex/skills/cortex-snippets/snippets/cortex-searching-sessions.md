---
name: cortex-searching-sessions
description: Search AI session history through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
inputs:
  query:
    type: string
    required: true
  limit:
    type: integer
    required: false
    default: 5
  since:
    type: string
    required: false
    default: 1h
tools:
  - cortex::cortex
---

# Search AI session history

Replacement for the Cortex `searching-sessions` skill's evidence collection. Start with the last hour; widen `since` deliberately for older work. Quote hyphenated FTS5 terms. Report project, host, session and timestamp; inspect source transcript before claiming prior work was completed.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "search_sessions", query: input.query, since: input.since ?? "1h", limit: Math.min(Math.max(input.limit ?? 5, 1), 10) };
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
      const sensitive = /best_snippet|^title$|message|text|content|stdout|stderr|transcript|command|token|secret|authorization|metadata|password|credential|api[_-]?key|private[_-]?key/i.test(key);
      if (key.length > 80) previewTruncated = true;
      result[key.slice(0, 80)] = sensitive && typeof value[key] !== "boolean" ? "[omitted]" : preview(value[key], depth + 1);
    }
    return result;
  };
  const bounded = preview(evidence);
  const serialized = JSON.stringify(bounded);
  return { ok: true, snippet: "cortex-searching-sessions", request: params,
    total_candidates: evidence.total_candidates, candidate_rows: evidence.candidate_rows,
    candidate_cap: evidence.candidate_cap, candidate_window_truncated: evidence.candidate_window_truncated,
    truncated: evidence.truncated,
    evidence_keys: Object.keys(bounded), evidence_preview: serialized.slice(0, 4000),
    preview_truncated: previewTruncated || serialized.length > 4000,
    guidance: 'This is a redacted, partial preview. Quote hyphenated FTS5 terms. Report project, host, session and timestamp; inspect source transcript before claiming prior work was completed.' };
}
```
