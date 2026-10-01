import { readFileSync } from "node:fs"
import { runInNewContext } from "node:vm"
import { describe, expect, it } from "vitest"

const source = readFileSync(new URL("../app/app.js", import.meta.url), "utf8")
const functions = source.slice(source.indexOf("  function normalizeHostSources"), source.indexOf("  function updateGraph"))
const { graphFromHosts, normalizeHostSources } = runInNewContext(`${functions}; ({ graphFromHosts, normalizeHostSources })`)

describe("host source graph", () => {
  it("preserves real API hostnames, aliases, and separate principal sources", () => {
    const hosts = [
      { hostname: "devhost", aliases: ["devhost", "devhost.local"], source_kind: "host" },
      { hostname: "agent-shared_bearer", source_kind: "forwarding_principal" },
      { hostname: "bearer-shared-Mac", source_kind: "forwarding_principal" },
      { hostname: "bearer-shared-mac", source_kind: "forwarding_principal" },
      { hostname: "devhost" }, {}, null,
    ]
    const graph = graphFromHosts(hosts)
    const nodes = graph.nodes.filter((node: { data: { id: string } }) => node.data.id.startsWith("source:"))
    expect(nodes.map((node: { data: { label: string } }) => node.data.label)).toEqual(["devhost", "agent-shared_bearer", "bearer-shared-Mac", "bearer-shared-mac"])
    expect(nodes[0].data.aliases).toEqual(["devhost", "devhost.local"])
    expect(nodes[1].data.kind).toBe("forwarding_principal")
    expect(nodes.every((node: { data: { status: string } }) => node.data.status === "observed")).toBe(true)
    expect(normalizeHostSources(hosts).filter((host: { source_kind: string }) => host.source_kind === "host")).toHaveLength(1)
  })

  it("supports legacy hostname strings without attributing shared credentials to a device", () => {
    expect(normalizeHostSources(["serverhost", "edgehost", "agent-shared_bearer", "bearer-shared-one", "localhost"]).map((host: { source_kind: string }) => host.source_kind))
      .toEqual(["host", "host", "forwarding_principal", "forwarding_principal", "unattributed"])
  })

  it("keeps claimed device names separate from heartbeat identities", () => {
    const sources = normalizeHostSources([
      { hostname: "edge-node-a", source_kind: "claimed_host", host_id: "stable-edge-node-a" },
      { hostname: "edge-node-a", source_kind: "host", host_id: "stable-edge-node-a" },
    ])
    expect(sources).toHaveLength(2)
    expect(sources[0].host_id).toBeNull()
    expect(sources[0].node_id).toBe("source:name:claimed_host:edge-node-a")
    expect(sources[1].host_id).toBe("stable-edge-node-a")
  })

  it("deduplicates physical nodes and counts using stable heartbeat IDs and merges their aliases", () => {
    const hosts = [
      { hostname: "serverhost", host_id: "device-1", source_kind: "host", aliases: ["SERVERHOST", "serverhost.local"] },
      { hostname: "serverhost.example.ts.net", host_id: "device-1", source_kind: "host", aliases: ["serverhost.local", "", null] },
      { hostname: "edgehost", host_id: "device-2", source_kind: "host" },
      { hostname: "agent-shared_bearer", host_id: "device-1", source_kind: "forwarding_principal" },
    ]
    const sources = normalizeHostSources(hosts)
    expect(sources.filter((host: { source_kind: string }) => host.source_kind === "host")).toHaveLength(2)
    expect(sources[0].aliases).toEqual(["serverhost", "SERVERHOST", "serverhost.local", "serverhost.example.ts.net"])
    const graph = graphFromHosts(hosts)
    const device = graph.nodes.find((node: { data: { host_id?: string } }) => node.data.host_id === "device-1")
    expect(device.data.id).toBe("source:heartbeat:device-1")
    expect(graph.edges.some((edge: { data: { source: string } }) => edge.data.source === device.data.id)).toBe(true)
    expect(sources.find((host: { source_kind: string }) => host.source_kind === "forwarding_principal").host_id).toBeNull()
    expect(graphFromHosts([hosts[1]]).nodes.find((node: { data: { host_id?: string } }) => node.data.host_id === "device-1").data.id).toBe(device.data.id)
  })

  it("retains ambiguous same-name devices and legacy rows without inventing identity", () => {
    const hosts = [
      { hostname: "same-name", host_id: "device-1", source_kind: "host" },
      { hostname: "same-name", host_id: "device-2", source_kind: "host" },
      { hostname: "same-name", host_id: null, source_kind: "host", aliases: ["legacy-alias"] },
      { hostname: "same-name", host_id: " ", source_kind: "host" },
      { hostname: "same-name", source_kind: "forwarding_principal" },
      "edgehost", "10.1.0.8",
    ]
    const sources = normalizeHostSources(hosts)
    expect(sources).toHaveLength(6)
    expect(sources.filter((host: { hostname: string }) => host.hostname === "same-name")).toHaveLength(4)
    expect(sources.find((host: { host_id?: string; source_kind: string }) => !host.host_id && host.source_kind === "host").aliases).toEqual(["same-name", "legacy-alias"])
    const graph = graphFromHosts(hosts)
    expect(new Set(graph.nodes.map((node: { data: { id: string } }) => node.data.id)).size).toBe(graph.nodes.length)
  })

  it("shows the stable heartbeat ID in selected device evidence", () => {
    const evidence: { children: { textContent: string }[] }[] = []
    const snippet = source.slice(source.indexOf("  function showNodeEvidence"), source.indexOf("  function renderTimeline"))
    const showNodeEvidence = runInNewContext(`${snippet}; showNodeEvidence`, {
      ui: { selectedTitle: {}, selectedKind: {}, evidenceList: { append: (item: { children: { textContent: string }[] }) => evidence.push(item) } },
      latestLogs: [], setBadge: () => {}, clear: () => {},
      text: (_tag: string, _className: string, textContent: string) => ({ textContent, children: [] as unknown[], append(...children: unknown[]) { this.children.push(...children) } }),
    })
    showNodeEvidence({ id: "source:heartbeat:device-1", label: "serverhost", kind: "host", host_id: "device-1" })
    expect(evidence.some((item) => item.children[0].textContent === "Heartbeat ID" && item.children[1].textContent === "device-1")).toBe(true)
  })
})
