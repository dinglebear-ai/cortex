#!/usr/bin/env bash
set -euo pipefail

jq empty docs/contracts/agent-observatory.schema.json
jq empty docs/contracts/agent-observatory.openapi.json
python3 scripts/check-agent-observatory-contracts.py

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT
sqlite3 "${tmp_dir}/observatory.db" < docs/contracts/agent-observatory.sql
sqlite3 "${tmp_dir}/observatory.db" 'PRAGMA integrity_check;' | grep -Fxq ok
rustc --edition 2024 --test docs/contracts/agent-observatory-types.rs -o "${tmp_dir}/agent-observatory-types-test"
"${tmp_dir}/agent-observatory-types-test"

if command -v tsc >/dev/null 2>&1; then
  tsc --noEmit --strict --target ES2022 --lib ES2022,DOM docs/contracts/agent-observatory-types.ts
fi

echo "agent observatory contract checks: ok"
