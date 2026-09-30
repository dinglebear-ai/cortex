import type { FreshnessLane } from "../../../docs/reference/contracts/agent-observatory-types"

export const disconnectedFreshnessLane = (): FreshnessLane => ({
  state: "not_observed",
  last_observed_at: null,
  lag_seconds: null,
  detail: "Cortex is not connected.",
})
