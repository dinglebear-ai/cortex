import AxeBuilder from "@axe-core/playwright"
import { expect, test } from "@playwright/test"

test("loads the disconnected Agent Observatory shell accessibly", async ({ page }) => {
  await page.goto("/app/agents/")

  await expect(page).toHaveTitle("Agent Observatory | Cortex")
  await expect(page.getByRole("main")).toBeVisible()
  await expect(page.getByRole("heading", { level: 1, name: "Agent Observatory" })).toBeVisible()

  const results = await new AxeBuilder({ page }).analyze()
  expect(results.violations).toEqual([])
})
