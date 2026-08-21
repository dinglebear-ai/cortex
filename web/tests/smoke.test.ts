import { describe, expect, it } from "vitest"

import { disconnectedFreshnessLane } from "../src/contracts/observatory"
// @ts-expect-error The production static server is intentionally plain JavaScript.
import { resolveStaticPath } from "../scripts/serve-static.mjs"

describe("Observatory contract helpers", () => {
  it("represents a disconnected data source without inventing observations", () => {
    expect(disconnectedFreshnessLane()).toEqual({
      state: "not_observed",
      last_observed_at: null,
      lag_seconds: null,
      detail: "Cortex is not connected.",
    })
  })
})

describe("static export path resolution", () => {
  it("maps the app mount into the configured static root", () => {
    expect(resolveStaticPath("/app/agents/", "/srv/cortex/out")).toBe(
      "/srv/cortex/out/agents",
    )
  })

  it("rejects malformed percent encoding", () => {
    expect(() => resolveStaticPath("/app/%zz", "/srv/cortex/out")).toThrow(
      "Malformed request URL",
    )
  })

  it("rejects encoded traversal outside the static root", () => {
    expect(() => resolveStaticPath("/app/%2e%2e%2f%2e%2e%2fsecrets", "/srv/cortex/out")).toThrow(
      "escapes static root",
    )
  })
})
