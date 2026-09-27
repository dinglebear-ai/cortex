---
name: cortex-searching-sessions
description: Search AI session history through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
inputs:
  query:
    type: string
    required: true
tools:
  - cortex::cortex
---

# Search AI session history

Replacement for the Cortex `searching-sessions` skill's evidence collection. Quote hyphenated FTS5 terms. Report project, host, session and timestamp; inspect source transcript before claiming prior work was completed.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "search_sessions", query: input.query };
  const evidence = await callTool("cortex::cortex", params);
  return { ok: true, snippet: "cortex-searching-sessions", request: params, evidence, guidance: 'Quote hyphenated FTS5 terms. Report project, host, session and timestamp; inspect source transcript before claiming prior work was completed.' };
}
```
