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
  return { ok: true, snippet: "cortex-incidents", request: params, evidence, guidance: 'Prioritize recent active signatures. Acknowledgement changes server state and requires a separate authorized action.' };
}
```
