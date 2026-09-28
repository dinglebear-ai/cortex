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
  return { ok: true, snippet: "cortex-skill-improvement-assessment", request: params, evidence, guidance: 'Use the evidence to assess triggering, instructions and observed agent behavior. Distinguish skill defects from tool failures or model noncompliance.' };
}
```
