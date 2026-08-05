#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contracts="$repo_root/docs/contracts"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'missing required command: %s\n' "$1" >&2
        exit 1
    }
}

for command_name in jq sqlite3 rustc node npm grep find; do
    require_command "$command_name"
done

jq empty \
    "$contracts/agent-observatory.schema.json" \
    "$contracts/agent-observatory.openapi.json" \
    "$contracts/agent-observatory-aurora-lock.example.json"
printf 'JSON contracts: ok\n'

contract_db="$tmp_dir/agent-observatory.db"
sql_output="$tmp_dir/sql-output.txt"
sqlite3 "$contract_db" <<SQL >"$sql_output"
.read $contracts/agent-observatory.sql
PRAGMA foreign_key_check;
PRAGMA integrity_check;
SQL

mapfile -t sql_lines <"$sql_output"
if [[ "${#sql_lines[@]}" -ne 1 || "${sql_lines[0]}" != "ok" ]]; then
    printf 'SQL contract validation failed:\n' >&2
    cat "$sql_output" >&2
    exit 1
fi
printf 'SQL integrity: ok\n'

rust_test_bin="$tmp_dir/agent-observatory-contract-tests"
rustc --edition=2024 --test \
    "$contracts/agent-observatory-types.rs" \
    -o "$rust_test_bin"
"$rust_test_bin"

cat >"$tmp_dir/tsconfig.json" <<JSON
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "lib": ["ES2022", "DOM"],
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true
  },
  "files": ["$contracts/agent-observatory-types.ts"]
}
JSON

run_tsc() {
    if [[ -n "${TSC:-}" ]]; then
        "$TSC" "$@"
    elif [[ -x "$repo_root/web/node_modules/.bin/tsc" ]]; then
        "$repo_root/web/node_modules/.bin/tsc" "$@"
    elif command -v tsc >/dev/null 2>&1; then
        tsc "$@"
    else
        npm exec --offline --yes --package=typescript@5.9.3 -- tsc "$@"
    fi
}

tsc_version="$(run_tsc --version)"
if [[ "$tsc_version" != "Version 5.9.3" ]]; then
    printf 'expected TypeScript 5.9.3, got: %s\n' "$tsc_version" >&2
    exit 1
fi
run_tsc --project "$tmp_dir/tsconfig.json"
printf 'TypeScript contracts: ok (%s)\n' "$tsc_version"

placeholder_pattern='TODO|TBD|FIXME|__APPEND__|REPLACE_AFTER'
placeholder_output="$tmp_dir/placeholders.txt"
: >"$placeholder_output"
while IFS= read -r path; do
    if [[ "$path" == *agent-observatory-aurora-lock.example.json ]]; then
        continue
    fi
    grep -nE "$placeholder_pattern" "$path" >>"$placeholder_output" || true
done < <(
    find "$repo_root/docs" -type f \
        \( -path '*/plans/agent-observatory/*' -o -name '*agent-observatory*' \) \
        -print | sort
)
if [[ -s "$placeholder_output" ]]; then
    printf 'unexpected Agent Observatory placeholders:\n' >&2
    cat "$placeholder_output" >&2
    exit 1
fi
printf 'Placeholder audit: ok\n'
