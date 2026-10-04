import { readFileSync } from "node:fs"
import { runInNewContext } from "node:vm"
import { describe, expect, it } from "vitest"

const source = readFileSync(new URL("../app/app.js", import.meta.url), "utf8")
const hostFunctions = source.slice(source.indexOf("  function normalizeHostSources"), source.indexOf("  function updateGraph"))
const { normalizeHostSources } = runInNewContext(`${hostFunctions}; ({ normalizeHostSources })`)

type Element = { textContent: string; children: Element[]; append: (...children: Element[]) => void }
const text = (_tag: string, _className: string, textContent: string): Element => ({
  textContent, children: [], append(...children) { this.children.push(...children) },
})

function inspect(latestLogs: object[], latestHosts: object[], selected: object) {
  const evidence: Element[] = []
  const snippet = source.slice(source.indexOf("  function showNodeEvidence"), source.indexOf("  function resetEvidenceInspector"))
  const showNodeEvidence = runInNewContext(`${snippet}; showNodeEvidence`, {
    ui: { selectedTitle: {}, selectedKind: {}, evidenceList: { append: (item: Element) => evidence.push(item) } },
    latestLogs, latestHosts, normalizeHostSources, setBadge: () => {}, clear: () => {}, text,
  })
  showNodeEvidence(selected)
  return evidence.flatMap((item) => [item.textContent, ...item.children.map((child) => child.textContent)]).join("\n")
}

const first = { id: "source:heartbeat:first", hostname: "shared", label: "shared", aliases: ["shared"], host_id: "first", source_kind: "host", kind: "host" }
const second = { ...first, id: "source:heartbeat:second", host_id: "second" }

describe("workspace evidence state", () => {
  it("does not invent fleet entities for an empty investigation graph", () => {
    const snippet = source.slice(source.indexOf("  function graphFromInvestigation"), source.indexOf("  function showNodeEvidence"))
    const build = runInNewContext(`${snippet}; graphFromInvestigation`)
    expect(build({ entities: [], relationships: [] })).toEqual({ nodes: [], edges: [] })
    expect(build(undefined)).toEqual({ nodes: [], edges: [] })
  })

  it("keeps same-name device evidence tied to its authoritative ID", () => {
    const output = inspect([
      { hostname: "shared", host_id: "second", message: "other device" },
      { hostname: "different-alias", host_id: "first", message: "selected device" },
    ], [first, second], first)
    expect(output).toContain("selected device")
    expect(output).not.toContain("other device")
  })

  it("omits ambiguous hostname-only rows instead of attributing them to a device", () => {
    const output = inspect([{ hostname: "shared", message: "ambiguous payload" }], [first, second], first)
    expect(output).toContain("name matches multiple sources")
    expect(output).not.toContain("ambiguous payload")
  })

  it("labels unique hostname-only evidence as unverified identity", () => {
    const output = inspect([{ hostname: "shared", message: "hostname payload" }], [first], first)
    expect(output).toContain("hostname payload")
    expect(output).toContain("device identity unverified")
  })
})
