import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.LIVE_PLAYWRIGHT_CORE);

const base = process.env.LIVE_CORTEX_URL;
const browser = await chromium.launch({executablePath: process.env.LIVE_BROWSER_EXECUTABLE, headless: true});
const page = await browser.newPage();
const consoleErrors = [];
const responseFailures = [];
const requestFailures = [];
page.on("console", message => { if (message.type() === "error") consoleErrors.push(message.text()); });
page.on("pageerror", error => consoleErrors.push(error.message));
page.on("response", response => {
  if (response.status() >= 400) responseFailures.push({status: response.status(), url: response.url()});
});
page.on("requestfailed", request => requestFailures.push({url: request.url(), error: request.failure()?.errorText || "unknown"}));
await page.goto(base + "/app", {waitUntil: "networkidle"});
await page.locator("#api-token").fill(process.env.LIVE_API_TOKEN);
await page.locator("[data-token-form]").evaluate(form => form.requestSubmit());
await page.waitForFunction(() => document.querySelector("[data-log-count]")?.textContent !== "--");
const connected = await page.locator("[data-server-version]").textContent();
const question = `What emitted logs for ${process.env.MCP_LIVE_HOST}?`;
await page.locator("#ask-input").fill(question);
const askResponsePromise = page.waitForResponse(response => response.url().endsWith("/api/v1/investigations/ask") && response.request().method() === "POST" && response.request().postDataJSON()?.prompt === question);
await page.locator("[data-ask-form]").evaluate(form => form.requestSubmit());
const askResponse = await askResponsePromise;
if (!askResponse.ok()) throw new Error(`Ask HTTP request failed: ${askResponse.status()}`);
const askEnvelope = await askResponse.json();
const expectedTitle = askEnvelope.result?.claims?.[0]?.title || question;
await page.waitForFunction(title => {
  const stack = document.querySelector("[data-answer-stack]");
  return stack?.dataset.askState === "complete" && [...stack.querySelectorAll("h2")].some(node => node.textContent === title);
}, expectedTitle);
if (await page.locator("[data-answer-stack]").getAttribute("data-ask-state") !== "complete") throw new Error("Ask request failed");
const rendered = await page.locator("[data-answer-stack]").textContent();
await page.locator("[data-clear-token]").click();
await page.locator("#api-token").fill("intentionally-wrong-token");
const authResponsePromise = page.waitForResponse(response =>
  response.url().endsWith("/api/version") &&
  response.request().headers()["authorization"] === "Bearer intentionally-wrong-token");
await page.locator("[data-token-form]").evaluate(form => form.requestSubmit());
const authResponse = await authResponsePromise;
if (![401, 403].includes(authResponse.status())) throw new Error(`Invalid token was not rejected: ${authResponse.status()}`);
await page.waitForFunction(() => document.querySelector("[data-answer-stack] h2")?.textContent === "Backend unavailable");
const authFailure = await page.locator("[data-answer-stack]").textContent();
await page.route("**/api/v1/investigations/ask", route => route.abort("failed"));
await page.locator("#api-token").fill(process.env.LIVE_API_TOKEN);
await page.locator("[data-token-form]").evaluate(form => form.requestSubmit());
await page.waitForFunction(() => document.querySelector("[data-answer-stack] h2")?.textContent === "Live workspace connected");
await page.locator("#ask-input").fill("force api failure");
const failedAskPromise = page.waitForEvent("requestfailed", request =>
  request.url().endsWith("/api/v1/investigations/ask") && request.postDataJSON()?.prompt === "force api failure");
await page.locator("[data-ask-form]").evaluate(form => form.requestSubmit());
await failedAskPromise;
await page.waitForFunction(() => {
  const stack = document.querySelector("[data-answer-stack]");
  return stack?.dataset.askState === "failed" && stack.querySelector("h2")?.textContent === "Ask failed";
});
const apiFailure = await page.locator("[data-answer-stack]").textContent();
const storage = await page.evaluate(() => ({local: {...localStorage}, session: {...sessionStorage}}));
process.stdout.write(JSON.stringify({connected, rendered, successfulQuery: true, authFailure, apiFailure, consoleErrors, responseFailures, requestFailures, storage}));
await browser.close();
