import AxeBuilder from "@axe-core/playwright"
import { expect, test, type Page } from "@playwright/test"

const hosts = [
  { hostname: "shared", aliases: ["shared"], source_kind: "host", host_id: "first" },
  { hostname: "shared", aliases: ["shared"], source_kind: "host", host_id: "second" },
  { hostname: "unique", aliases: ["unique"], source_kind: "host", host_id: "unique" },
]

async function mockApi(page: Page) {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname
    expect(route.request().headers().authorization).toMatch(/^Bearer .+fixture-token$/)
    const responses: Record<string, object> = {
      "/api/v1/investigation/version": { ui_version: "investigation-v1" },
      "/api/version": { version: "browser-fixture", schema_version: 1 },
      "/api/stats": { total_logs: 3 },
      "/api/hosts": { hosts },
      "/api/tail": { logs: [
        { hostname: "shared", message: "Ambiguous private log" },
        { hostname: "unique", message: "Unique private log" },
      ] },
      "/api/v1/investigations/ask": { result: { claims: [], logs: [], graph: { entities: [], relationships: [] } }, metadata: { partial: false } },
    }
    await route.fulfill({ json: responses[path] ?? {}, status: responses[path] ? 200 : 404 })
  })
}

async function connect(page: Page, token = "browser-fixture-token") {
  await page.getByLabel("Bearer token").fill(token)
  await page.getByRole("button", { name: "Connect", exact: true }).click()
  await expect(page.locator("[data-log-status]")).toHaveText("Live")
}

// Use the graph's registered instance only to locate the node; selection itself
// is a real pointer click against the production Cytoscape canvas.
async function selectNode(page: Page, id: string) {
  await page.locator("#graph-canvas").scrollIntoViewIfNeeded()
  const position = await page.locator("#graph-canvas").evaluate((element, nodeId) => {
    const canvas = element as HTMLElement & { _cyreg: { cy: { getElementById: (id: string) => { renderedPosition: () => { x: number; y: number } } } } }
    const position = canvas._cyreg.cy.getElementById(nodeId).renderedPosition()
    return position
  }, id)
  // Locator click waits for a stable, actionable canvas after scrolling.
  // Raw mouse coordinates can race the canvas scroll/paint update.
  await page.locator("#graph-canvas").click({ position })
}

test("serves the production disconnected workspace accessibly", async ({ page }) => {
  await page.goto("/app/investigate")
  await expect(page).toHaveTitle("Cortex investigation workspace")
  await expect(page.getByRole("heading", { level: 1, name: "Cortex investigation workspace" })).toBeVisible()
  await expect(page.getByLabel("Bearer token")).toBeVisible()
  await expect(page.locator("[data-log-status]")).toHaveText("Preview")
  const results = await new AxeBuilder({ page }).analyze()
  expect(results.violations).toEqual([])
})

test("inspector avoids ambiguous device evidence and clears on disconnect and reconnect", async ({ page }) => {
  await mockApi(page)
  await page.goto("/app/investigate")
  await connect(page)
  await selectNode(page, "source:heartbeat:first")
  await expect(page.locator("[data-evidence-list]")).toContainText("name matches multiple sources")
  await expect(page.locator("[data-evidence-list]")).not.toContainText("Ambiguous private log")

  await selectNode(page, "source:heartbeat:unique")
  await expect(page.locator("[data-evidence-list]")).toContainText("Unique private log")
  await expect(page.locator("[data-evidence-list]")).toContainText("device identity unverified")
  await page.getByRole("button", { name: "Clear", exact: true }).click()
  await expect(page.getByLabel("Bearer token")).toHaveValue("")
  await expect(page.locator("[data-selected-title]")).toHaveText("Nothing selected")
  await expect(page.locator("[data-evidence-list]")).not.toContainText("Unique private log")

  await connect(page)
  await selectNode(page, "source:heartbeat:unique")
  await expect(page.locator("[data-evidence-list]")).toContainText("Unique private log")
  await connect(page, "replacement-fixture-token")
  await expect(page.locator("[data-selected-title]")).toHaveText("Nothing selected")
  await expect(page.locator("[data-evidence-list]")).not.toContainText("Unique private log")
})

test("empty successful investigation has an explicit empty graph", async ({ page }) => {
  await mockApi(page)
  await page.goto("/app/investigate")
  await connect(page)
  await page.getByLabel("Ask Cortex").fill("An investigation with no graph evidence")
  await page.getByRole("button", { name: "Ask", exact: true }).click()
  await expect(page.locator("[data-answer-stack]")).toHaveAttribute("data-ask-state", "complete")
  await expect(page.locator("[data-evidence-list]")).toContainText("No graph entities returned")
  const count = await page.locator("#graph-canvas").evaluate((element) => {
    const canvas = element as HTMLElement & { _cyreg: { cy: { nodes: () => { length: number } } } }
    return canvas._cyreg.cy.nodes().length
  })
  expect(count).toBe(0)
})

for (const action of ["clear", "replace token"] as const) {
  test(`${action} invalidates a delayed authenticated response`, async ({ page }) => {
    await mockApi(page)
    let release: (() => void) | undefined
    const gate = new Promise<void>((resolve) => { release = resolve })
    await page.route("**/api/v1/investigations/ask", async (route) => {
      expect(route.request().headers().authorization).toBe("Bearer browser-fixture-token")
      await gate
      await route.fulfill({ json: { result: { claims: [{ title: "Stale private answer", summary: "Must remain hidden" }], logs: [], graph: { entities: [], relationships: [] } }, metadata: {} } })
    })
    await page.goto("/app/investigate")
    await connect(page)
    await page.getByLabel("Ask Cortex").fill("Delayed evidence")
    const requested = page.waitForRequest("**/api/v1/investigations/ask")
    await page.getByRole("button", { name: "Ask", exact: true }).click()
    await requested
    if (action === "clear") {
      await page.getByRole("button", { name: "Clear", exact: true }).click()
    } else {
      await connect(page, "replacement-fixture-token")
    }
    const responded = page.waitForResponse("**/api/v1/investigations/ask")
    release?.()
    await (await responded).finished()
    // Let the consumed response and its promise continuation settle before
    // asserting that the old credentials cannot repopulate the workspace.
    await page.evaluate(() => new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    }))
    await expect(page.locator("[data-answer-stack]")).toHaveAttribute("data-ask-state", "idle")
    await expect(page.locator("[data-answer-stack]")).not.toContainText("Stale private answer")
    await expect(page.locator("[data-log-status]")).toHaveText(action === "clear" ? "Disconnected" : "Live")
  })
}
