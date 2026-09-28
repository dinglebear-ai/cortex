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
For an identified session, call the direct Cortex `session_investigate` action with its `session_id` and known `tool`, `project`, and `host`. Inspect partial-result flags; transcript-derived references remain claims, not verified completion.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "search_sessions", query: input.query, since: input.since ?? "1h", limit: Math.min(Math.max(input.limit ?? 5, 1), 10) };
  const evidence = await callTool("cortex::cortex", params);
  return { ok: true, snippet: "cortex-searching-sessions", request: params, evidence_keys: Object.keys(evidence), evidence_preview: JSON.stringify(evidence, (key, value) => { if (/message|text|content|stdout|stderr|transcript|command|token|secret|authorization/i.test(key)) return "[omitted]"; if (typeof value === "string") return value.slice(0, 250); if (Array.isArray(value)) return value.slice(0, 3); return value; }).slice(0, 4000), guidance: 'Quote hyphenated FTS5 terms. Report project, host, session and timestamp; inspect source transcript before claiming prior work was completed.' };
}
```
