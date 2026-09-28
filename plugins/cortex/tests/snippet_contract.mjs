import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const snippets = new URL("../skills/cortex-snippets/snippets/", import.meta.url);

function loadSnippet(name, callTool, codemode = {}) {
  const path = new URL(`${name}.md`, snippets);
  const source = readFileSync(fileURLToPath(path), "utf8");
  const code = source.match(/```js\n([\s\S]*?)\n```/)?.[1];
  assert.ok(code, `${name} has one JavaScript body`);
  return Function("callTool", "codemode", `return (${code})`)(callTool, codemode);
}

const largeEvidence = {
  incidents: [{ message: "private-message", transcript: "private-transcript", metadata: "x".repeat(1_500_000) }],
};
const skill = await loadSnippet("cortex-skill-improvement-assessment", async (id, params) => {
  assert.equal(id, "cortex::cortex");
  assert.deepEqual(params, { action: "skill_investigate", skill: "example-skill" });
  return largeEvidence;
})({ skill: "example-skill" });
assert.equal(skill.ok, true);
assert.ok(skill.evidence_preview.length <= 4000);
assert.ok(!JSON.stringify(skill).includes("private-message"));
assert.ok(!JSON.stringify(skill).includes("private-transcript"));
assert.ok(!JSON.stringify(skill).includes("x".repeat(100)));

const calls = [];
const topology = loadSnippet("cortex-topology", async (id, params) => {
  assert.equal(id, "cortex::cortex");
  calls.push(params);
  if (params.action === "hosts") {
    return { hosts: [{ hostname: "macpoo", last_seen: "2026-09-27T00:00:00Z", log_count: 12 }] };
  }
  if (params.action === "apps") {
    return { apps: [{ app_name: "app", log_count: 3, last_seen: "2026-09-27T00:00:00Z" }], total: 1 };
  }
  throw Error("unexpected action");
}, { batch: async tasks => {
  const values = await Promise.all(tasks.map(task => task()));
  return { all_ok: true, ok: values.map((value, i) => ({ i, value })), failed: [] };
} });
const knownHost = await topology({ host: "macpoo" });
assert.equal(knownHost.ok, true);
assert.equal(knownHost.source, "log_activity");
assert.deepEqual(knownHost.apps.map((app) => app.name), ["app"]);
assert.deepEqual(calls, [{ action: "hosts" }, { action: "apps", host: "macpoo", since: "1h", limit: 20 }]);
assert.match(knownHost.coverage, /neither running-service status nor graph-backed dependencies/);
assert.equal((await topology({ host: "missing" })).ok, false);

let searchParams;
const search = loadSnippet("cortex-searching-sessions", async (id, params) => {
  assert.equal(id, "cortex::cortex");
  searchParams = params;
  return { sessions: [] };
});
assert.equal((await search({ query: '"install-cortex"', limit: 100 })).ok, true);
assert.deepEqual(searchParams, { action: "search_sessions", query: '"install-cortex"', since: "1h", limit: 10 });
await search({ query: "older work", since: "7d", limit: 0 });
assert.equal(searchParams.since, "7d");
assert.equal(searchParams.limit, 1);

console.log("Cortex snippet contracts passed");
