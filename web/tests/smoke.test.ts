import { describe, expect, it } from "vitest"

import { disconnectedFreshnessLane } from "../src/contracts/observatory"

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
