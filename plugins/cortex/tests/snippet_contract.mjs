import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const snippets = new URL("../skills/cortex-snippets/snippets/", import.meta.url);

function loadSnippet(name, callTool) {
  const path = new URL(`${name}.md`, snippets);
  const source = readFileSync(fileURLToPath(path), "utf8");
  const code = source.match(/```js\n([\s\S]*?)\n```/)?.[1];
  assert.ok(code, `${name} has one JavaScript body`);
  return Function("callTool", "codemode", `return (${code})`)(callTool, {});
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

const inventory = {
  nodes: [{ hostname: "macpoo", last_seen: "2026-09-27T00:00:00Z", apps: [] }],
  services: [
    { host: "http://macpoo.example:2375", name: "app", kind: "docker_container", status: "running" },
    { host: "other", name: "unrelated", kind: "docker_container", status: "running" },
  ],
  freshness: { is_stale: false },
};
const topology = loadSnippet("cortex-topology", async (id, params) => {
  assert.equal(id, "cortex::cortex");
  assert.deepEqual(params, { action: "map", mode: "snapshot" });
  return inventory;
});
const knownHost = await topology({ host: "macpoo" });
assert.equal(knownHost.ok, true);
assert.equal(knownHost.source, "inventory_snapshot");
assert.deepEqual(knownHost.services.map((service) => service.name), ["app"]);
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
