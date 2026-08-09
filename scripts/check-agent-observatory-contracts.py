#!/usr/bin/env python3
"""Cross-artifact golden validation for the proposed Agent Observatory contract."""

import json
import re
import sys
from pathlib import Path

from jsonschema import Draft202012Validator, FormatChecker, RefResolver

ROOT = Path(__file__).resolve().parents[1]
CONTRACTS = ROOT / "docs/contracts"
SCHEMA_PATH = CONTRACTS / "agent-observatory.schema.json"
OPENAPI_PATH = CONTRACTS / "agent-observatory.openapi.json"
FIXTURE_PATH = CONTRACTS / "fixtures/agent-observatory-golden.json"


def fail(message: str) -> None:
    raise AssertionError(message)


schema = json.loads(SCHEMA_PATH.read_text())
openapi = json.loads(OPENAPI_PATH.read_text())
fixture = json.loads(FIXTURE_PATH.read_text())
resolver = RefResolver(base_uri=SCHEMA_PATH.as_uri(), referrer=schema)


def validate_schema(name: str, value: object) -> None:
    candidate = {"$ref": f"#/$defs/{name}", "$defs": schema["$defs"]}
    Draft202012Validator(candidate, resolver=resolver, format_checker=FormatChecker()).validate(value)


validate_schema("AgentRunDetail", fixture["run_detail"])
validate_schema("AgentRunEvent", fixture["event"])
validate_schema("StreamEnvelope", fixture["stream"])
validate_schema("Page", fixture["terminal_page"])
validate_schema("Page", fixture["continued_page"])

telemetry_schema = openapi["components"]["schemas"]["Telemetry"]
store = {SCHEMA_PATH.as_uri(): schema, OPENAPI_PATH.as_uri(): openapi}
Draft202012Validator(
    telemetry_schema,
    resolver=RefResolver(base_uri=OPENAPI_PATH.as_uri(), referrer=openapi, store=store),
    format_checker=FormatChecker(),
).validate(fixture["telemetry"])

backfill_schema = openapi["components"]["schemas"]["BackfillJob"]
Draft202012Validator(
    backfill_schema,
    resolver=RefResolver(base_uri=OPENAPI_PATH.as_uri(), referrer=openapi, store=store),
    format_checker=FormatChecker(),
).validate(fixture["backfill_job"])

detail_ref = openapi["paths"]["/api/agent-runs/{run_key}"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["$ref"]
if not detail_ref.endswith("#/$defs/AgentRunDetail"):
    fail("OpenAPI run detail does not reference AgentRunDetail")

reconcile = openapi["paths"]["/api/agent-observatory/reconcile"]["post"]["requestBody"]["content"]["application/json"]["schema"]
reconcile_validator = Draft202012Validator(reconcile, resolver=RefResolver(base_uri=OPENAPI_PATH.as_uri(), referrer=openapi, store=store))
for accepted in ({"repository_id": "1"}, {"all": True}, {"repository_id": "1", "dry_run": True}):
    reconcile_validator.validate(accepted)
for rejected in ({}, {"all": False}, {"repository_id": "1", "all": True}):
    if reconcile_validator.is_valid(rejected):
        fail(f"reconcile unexpectedly accepted {rejected}")

backfill = openapi["paths"]["/api/agent-observatory/backfill"]["post"]["requestBody"]["content"]["application/json"]["schema"]
if backfill.get("x-cortex-validation") != "decimal(from_id) <= decimal(until_id)":
    fail("backfill range rule is absent")
for accepted in ({"source": "logs"}, {"source": "logs", "from_id": "10", "until_id": "20"}):
    if "from_id" in accepted and int(accepted["from_id"]) > int(accepted["until_id"]):
        fail("invalid positive fixture")
for rejected in ({"source": "logs", "from_id": "21", "until_id": "20"},):
    if int(rejected["from_id"]) <= int(rejected["until_id"]):
        fail("invalid negative fixture")

for route, verb in (("/api/agent-observatory/backfill/{job_id}", "get"), ("/api/agent-observatory/backfill/{job_id}", "delete"), ("/api/agent-observatory/backfill/{job_id}/restart", "post")):
    if verb not in openapi["paths"].get(route, {}):
        fail(f"missing lifecycle route {verb.upper()} {route}")

for name in ("RepositoryPage", "WorktreePage", "RunPage", "EventPage", "Telemetry", "Status", "Accepted", "BackfillJob", "Error"):
    if openapi["components"]["schemas"][name].get("additionalProperties") is not False:
        fail(f"response component {name} is not strict")

bad_detail = dict(fixture["run_detail"], unexpected=True)
if Draft202012Validator({"$ref": "#/$defs/AgentRunDetail", "$defs": schema["$defs"]}).is_valid(bad_detail):
    fail("AgentRunDetail accepts unknown fields")
bad_page = {"limit": 50, "truncated": False}
if Draft202012Validator({"$ref": "#/$defs/Page", "$defs": schema["$defs"]}).is_valid(bad_page):
    fail("Page accepts missing next_cursor")

rust = (CONTRACTS / "agent-observatory-types.rs").read_text()
typescript = (CONTRACTS / "agent-observatory-types.ts").read_text()
for token in ("pub run: AgentRunSummary", "pub commit_summary: Option<GitCommitSummary>", "pub payload: Option<JsonObject>", "pub data: JsonObject"):
    if token not in rust:
        fail(f"Rust contract missing {token}")
for token in ("run: AgentRunSummary", "commit_summary: GitCommitSummary | null", "payload: TPayload | null", "data: TData", "freshness: RunFreshness"):
    if token not in typescript:
        fail(f"TypeScript contract missing {token}")

print("agent observatory golden contracts: ok")
