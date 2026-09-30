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
    expect(normalizeHostSources(["serverhost", "agent-os", "agent-shared_bearer", "bearer-shared-one", "localhost"]).map((host: { source_kind: string }) => host.source_kind))
      .toEqual(["host", "host", "forwarding_principal", "forwarding_principal", "unattributed"])
  })
})
