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

Replacement for the Cortex `topology` skill's evidence collection. Use the inventory snapshot for host services, since a log host may have no corresponding graph entity. This is observed inventory, not graph-proven dependency evidence. Use `using-cortex` for a separate graph query when dependencies matter.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const params = { action: "map", mode: "snapshot" };
  const snapshot = await callTool("cortex::cortex", params);
  const host = input.host.trim().toLowerCase();
  const node = snapshot.nodes?.find(item => item.hostname?.toLowerCase() === host);
  const services = (snapshot.services ?? []).filter(item => {
    const source = String(item.host ?? "").toLowerCase();
    return source === host || source.includes("://" + host + ".") || source.includes("://" + host + ":");
  }).slice(0, 20).map(item => ({ name: item.name, kind: item.kind, status: item.status }));
  return { ok: Boolean(node || services.length), snippet: "cortex-topology", source: "inventory_snapshot", request: params,
    host: input.host, node: node && { hostname: node.hostname, last_seen: node.last_seen, apps: (node.apps ?? []).slice(0, 10) },
    services, freshness: snapshot.freshness, cache_status: snapshot.cache_status,
    coverage: "Inventory services are observed sources, not graph-proven dependencies." };
}
```
