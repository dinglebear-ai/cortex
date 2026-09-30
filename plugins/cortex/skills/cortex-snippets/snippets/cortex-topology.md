---
name: cortex-topology
description: Inspect a host and its recently observed apps through Labby Code Mode and the Cortex MCP upstream.
tags: [cortex, readonly]
inputs:
  host:
    type: string
    required: true
  since:
    type: string
    required: false
    default: 1h
tools:
  - cortex::cortex
---

# Inspect a host and observed apps

Replacement for the Cortex `topology` skill's quick host evidence. It lists a known log host and apps observed in its logs during a bounded window. This cannot establish running service status or graph-backed dependencies; use `using-cortex` for inventory or graph questions that need those claims.
Before first use, discover `cortex::cortex` with `codemode.search()` and inspect its live schema with `codemode.describe("cortex.cortex")`; adapt fields if the server contract changed.

```js
async (input) => {
  const since = input.since ?? "1h";
  const host = input.host.trim().replace(/\.+$/, "").toLowerCase();
  const requests = [{ action: "hosts" }, { action: "apps", host, since, limit: 20 }];
  const batch = await codemode.batch(requests.map(params => () => callTool("cortex::cortex", params)));
  const hosts = batch.ok.find(item => item.i === 0)?.value.hosts ?? [];
  const appResult = batch.ok.find(item => item.i === 1)?.value;
  const node = hosts.find(item => item.hostname?.trim().replace(/\.+$/, "").toLowerCase() === host);
  return { ok: batch.all_ok && Boolean(node), snippet: "cortex-topology", source: "log_activity",
    host: node && { hostname: node.hostname, last_seen: node.last_seen, log_count: node.log_count }, since,
    apps: (appResult?.apps ?? []).slice(0, 20).map(item => ({ name: item.app_name, log_count: item.log_count, last_seen: item.last_seen })),
    total_apps: appResult?.total,
    failures: batch.failed.map(item => ({ source: ["hosts", "apps"][item.i], error: "Upstream call failed; inspect the recorded tool failure." })),
    coverage: "Recent log activity only; neither running-service status nor graph-backed dependencies are proven." };
}
```
