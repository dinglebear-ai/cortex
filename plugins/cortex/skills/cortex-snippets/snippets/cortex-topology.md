---
name: cortex-topology
description: Inspect a host in the Cortex topology through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
inputs:
  host:
    type: string
    required: true
tools:
  - cortex::cortex
---

# Inspect a host in the Cortex topology

Replacement for the Cortex `topology` skill's evidence collection. Use mode=host_services to inspect observed services. Report freshness and source coverage before inferring dependencies.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "map", mode: "host_services", host: input.host };
  const evidence = await callTool("cortex::cortex", params);
  return { ok: true, snippet: "cortex-topology", request: params, evidence, guidance: 'Use mode=host_services to inspect observed services. Report freshness and source coverage before inferring dependencies.' };
}
```
