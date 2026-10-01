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
  evidence: [{ incident: { incident_id: "skill-1", skill_name: "example-skill" },
    nearby_logs: [{ message: "private-message", transcript: "private-transcript", metadata: "x".repeat(1_500_000) }],
    transcript_before_truncated: true }],
  total_incidents: 4, truncated: true, no_data: false,
  no_incident_low_severity_summary: false,
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
assert.equal(skill.total_incidents, 4);
assert.equal(skill.truncated, true);
assert.equal(skill.no_data, false);
assert.equal(JSON.parse(skill.evidence_preview).evidence[0].transcript_before_truncated, true);

const wideEvidence = Object.fromEntries(Array.from({ length: 2000 }, (_, i) => [`key${i}`, "v".repeat(500)]));
wideEvidence.total_incidents = 9;
wideEvidence.truncated = true;
const wide = await loadSnippet("cortex-skill-improvement-assessment", async () => wideEvidence)({ skill: "wide" });
assert.ok(wide.evidence_keys.length <= 20);
assert.ok(wide.evidence_keys.every(key => key.length <= 80));
assert.ok(wide.evidence_preview.length <= 4000);
assert.equal(wide.preview_truncated, true);
assert.equal(wide.total_incidents, 9);
assert.equal(wide.truncated, true);
assert.ok(JSON.stringify(wide).length < 5000);

let deepEvidence = { password: "private-password" };
for (let i = 0; i < 1000; i++) deepEvidence = { nested: deepEvidence };
const deep = await loadSnippet("cortex-skill-improvement-assessment", async () => deepEvidence)({ skill: "deep" });
assert.equal(deep.preview_truncated, true);
assert.ok(deep.evidence_preview.length < 500);
assert.ok(!JSON.stringify(deep).includes("private-password"));

let visited = 0;
const branches = (depth) => depth === 0 ? "leaf" : Object.fromEntries(Array.from({ length: 20 }, (_, i) => {
  const branch = {};
  Object.defineProperty(branch, "child", { enumerable: true, get() { visited++; return branches(depth - 1); } });
  return [`branch${i}`, branch];
}));
const bounded = await loadSnippet("cortex-skill-improvement-assessment", async () => branches(4))({ skill: "branching" });
assert.ok(visited <= 120, `bounded traversal visited ${visited} branches`);
assert.equal(bounded.preview_truncated, true);

const noData = await loadSnippet("cortex-skill-improvement-assessment", async () => ({
  evidence: [], total_incidents: 0, truncated: false, no_data: true,
  suggested_filters: ["widen since"],
}))({ skill: "unknown" });
assert.equal(noData.no_data, true);
assert.equal(noData.total_incidents, 0);
assert.equal(noData.preview_truncated, false);
await assert.rejects(loadSnippet("cortex-skill-improvement-assessment", async () => { throw Error("upstream failure"); })({ skill: "failed" }));

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
const knownHost = await topology({ host: "  MACPOO.  " });
assert.equal(knownHost.ok, true);
assert.equal(knownHost.source, "log_activity");
assert.deepEqual(knownHost.apps.map((app) => app.name), ["app"]);
assert.deepEqual(calls, [{ action: "hosts" }, { action: "apps", host: "macpoo", since: "1h", limit: 20 }]);
assert.match(knownHost.coverage, /neither running-service status nor graph-backed dependencies/);
assert.equal((await topology({ host: "missing" })).ok, false);
const partial = await loadSnippet("cortex-topology", async () => {}, { batch: async () => ({
  all_ok: false,
  ok: [{ i: 0, value: { hosts: [{ hostname: "macpoo", log_count: 12 }] } }],
  failed: [{ i: 1, error: Error("Authorization: Bearer private-token") }],
}) })({ host: "macpoo" });
assert.equal(partial.ok, false);
assert.equal(partial.host.hostname, "macpoo");
assert.deepEqual(partial.apps, []);
assert.equal(partial.failures[0].source, "apps");
assert.ok(!JSON.stringify(partial).includes("private-token"));
const failedHosts = await loadSnippet("cortex-topology", async () => {}, { batch: async () => ({
  all_ok: false, ok: [{ i: 1, value: { apps: [{ app_name: "app" }], total: 2 } }],
  failed: [{ i: 0, error: Error("unavailable") }],
}) })({ host: "macpoo" });
assert.equal(failedHosts.ok, false);
assert.equal(failedHosts.host, undefined);
assert.equal(failedHosts.total_apps, 2);
assert.equal(failedHosts.failures[0].source, "hosts");

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
await assert.rejects(loadSnippet("cortex-searching-sessions", async () => { throw Error("upstream failure"); })({ query: "failed" }));

const searchEvidence = { sessions: Array.from({ length: 10 }, (_, i) => ({ session_id: `session${i}`, hostname: "host", best_snippet: "private-search-message", title: "private-session-title", match_count: 10 })), total_candidates: 100,
  candidate_rows: 100, candidate_cap: 1000, candidate_window_truncated: false, truncated: true };
const searchResult = await loadSnippet("cortex-searching-sessions", async () => searchEvidence)({ query: "history" });
assert.equal(searchResult.preview_truncated, true);
assert.equal(searchResult.total_candidates, 100);
assert.equal(searchResult.candidate_rows, 100);
assert.equal(searchResult.candidate_cap, 1000);
assert.equal(searchResult.candidate_window_truncated, false);
assert.equal(searchResult.truncated, true);
assert.ok(!JSON.stringify(searchResult).includes("private-search-message"));
assert.ok(!JSON.stringify(searchResult).includes("private-session-title"));
const wideSearch = await loadSnippet("cortex-searching-sessions", async () => wideEvidence)({ query: "wide" });
assert.ok(wideSearch.evidence_keys.length <= 20);
assert.ok(wideSearch.evidence_preview.length <= 4000);
assert.equal(wideSearch.preview_truncated, true);
const deepSearch = await loadSnippet("cortex-searching-sessions", async () => deepEvidence)({ query: "deep" });
assert.equal(deepSearch.preview_truncated, true);
assert.ok(deepSearch.evidence_preview.length < 500);

const redactionEvidence = { password: "private-password", credential: "private-credential", metadata: { url: "private-metadata" },
  evidence: [{ transcript_before_truncated: true, text: "private-transcript" }], total_incidents: 7, truncated: true };
for (const [name, input] of [
  ["cortex-frustration-assessment", { incident_id: "one" }],
  ["cortex-hook-friction-assessment", { hook_name: "hook" }],
  ["cortex-mcp-friction-assessment", { mcp_server: "server" }],
  ["cortex-incidents", { limit: 5 }],
]) {
  const run = loadSnippet(name, async () => redactionEvidence);
  const result = await run(input);
  assert.ok(!JSON.stringify(result).includes("private-"), `${name} redacts credential and transcript values`);
  assert.equal(result.truncated, true);
  assert.equal(JSON.parse(result.evidence_preview).evidence[0].transcript_before_truncated, true);
  const broad = await loadSnippet(name, async () => wideEvidence)(input);
  assert.equal(broad.preview_truncated, true);
  assert.ok(broad.evidence_keys.length <= 20);
  assert.ok(broad.evidence_preview.length <= 4000);
  const nested = await loadSnippet(name, async () => deepEvidence)(input);
  assert.equal(nested.preview_truncated, true);
  assert.ok(nested.evidence_preview.length < 500);
}
for (const name of ["cortex-report", "cortex-troubleshoot"]) {
  const run = loadSnippet(name, async () => {}, { batch: async () => ({
    all_ok: false,
    ok: [{ i: 0, value: redactionEvidence }],
    failed: [{ i: 1, error: Error("Authorization: Bearer private-token") }],
  }) });
  const result = await run({});
  assert.equal(result.ok, false);
  assert.ok(!JSON.stringify(result).includes("private-"), `${name} redacts values and upstream failures`);
  const broad = await loadSnippet(name, async () => {}, { batch: async () => ({
    all_ok: true, ok: [{ i: 0, value: wideEvidence }], failed: [],
  }) })({});
  assert.equal(broad.results[0].preview_truncated, true);
  assert.ok(broad.results[0].preview.length <= 1800);
}

const actualMcpEvidence = { evidence: [{ mcp_events: [{
  arguments_json: '{"env":"private-argument"}', output_preview: "private-tool-output",
  input: { api_key: "private-input" }, output: "private-output", is_error: false,
}] }] };
for (const [name, input] of [
  ["cortex-frustration-assessment", { incident_id: "one" }],
  ["cortex-hook-friction-assessment", { hook_name: "hook" }],
  ["cortex-mcp-friction-assessment", { mcp_server: "server" }],
  ["cortex-incidents", { limit: 5 }],
  ["cortex-skill-improvement-assessment", { skill: "skill" }],
  ["cortex-searching-sessions", { query: "query" }],
]) {
  const result = await loadSnippet(name, async () => actualMcpEvidence)(input);
  assert.ok(!JSON.stringify(result).includes("private-"), `${name} omits actual MCP argument/input/output fields`);
  assert.equal(JSON.parse(result.evidence_preview).evidence[0].mcp_events[0].is_error, false);
}
for (const name of ["cortex-report", "cortex-troubleshoot"]) {
  const result = await loadSnippet(name, async () => {}, { batch: async () => ({
    all_ok: true, ok: [{ i: 0, value: actualMcpEvidence }], failed: [],
  }) })({});
  assert.ok(!JSON.stringify(result).includes("private-"), `${name} omits actual MCP argument/input/output fields`);
}

const validator = readFileSync(fileURLToPath(new URL("../../../scripts/validate-marketplace.sh", import.meta.url)), "utf8");
assert.match(validator, /python3 -m unittest discover -s plugins\/cortex\/tests/);
const ci = readFileSync(fileURLToPath(new URL("../../../.github/workflows/ci.yml", import.meta.url)), "utf8");
assert.ok(ci.split("\n  version-sync:")[1].split("\n  identity:")[0].includes("bash scripts/validate-marketplace.sh"));
console.log("Cortex snippet contracts passed");
