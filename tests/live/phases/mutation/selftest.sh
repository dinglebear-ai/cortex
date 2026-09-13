#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../../../.." && pwd)"
jq -e '.production_runtime_switches==false and (.mutants|length)==9 and ([.mutants[].id]|unique|length)==9' "$root/tests/live/phases/mutation/mutants.json" >/dev/null
python3 "$root/tests/live/phases/mutation/test_driver.py"
switch_scan=0; grep -rEnq 'CORTEX_.*MUTANT|MUTANT_ID' "$root/src" || switch_scan=$?
[[ "$switch_scan" == 1 ]] || { echo "mutant switch found in src, or the scan failed (grep status $switch_scan)" >&2; exit 1; }
echo 'mutation selftest: PASS'
