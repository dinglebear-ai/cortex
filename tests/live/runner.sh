#!/usr/bin/env bash
set -euo pipefail
LIVE_PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"; export LIVE_PROJECT_ROOT
# shellcheck disable=SC1090
for lib in common lock redact events command lease resources report artifacts contracts budgets; do source "$LIVE_PROJECT_ROOT/tests/live/lib/$lib.sh"; done

usage() { echo "usage: tests/live/runner.sh [--profile noop|smoke|full|soak] [--runs-root DIR] [--janitor] [--provider ID] [--target ID] [--legacy ARGS...]"; }
profile=smoke; runs_root="${TMPDIR:-/tmp}/cortex-live-runs"; janitor=false; provider="local:$LIVE_PROJECT_ROOT"; target="local"; legacy=false; legacy_args=(); legacy_runner="${LIVE_LEGACY_RUNNER:-$LIVE_PROJECT_ROOT/tests/test_live.sh}"
while (($#)); do case "$1" in --profile) profile="$2"; shift 2;; --runs-root) runs_root="$2"; shift 2;; --provider) provider="$2"; shift 2;; --target) target="$2"; shift 2;; --janitor) janitor=true; shift;; --legacy) legacy=true; shift; legacy_args=("$@"); break;; -h|--help) usage; exit;; *) usage >&2; exit 2;; esac; done
live_require_tools bash jq openssl cargo shasum ps pgrep find stat sed awk || live_die "live harness prerequisites unavailable"
jq -e --arg p "$profile" '.profiles[$p]' "$LIVE_PROJECT_ROOT/tests/live/contracts/profiles.json" >/dev/null || live_die "unknown profile: $profile"
if $janitor; then live_janitor "$runs_root" "$provider"; exit; fi
live_init_run "$runs_root" >/dev/null
live_contract_export "$LIVE_RUN_ROOT/surface-contract.json"
live_run_manifest_write "$profile" "$provider" "$target" "$LIVE_SURFACE_CONTRACT"
live_budget_start
live_lease_write 120
live_runner_cleanup() {
  local status=$?
  trap - HUP INT TERM EXIT
  live_cleanup_resources "$provider" >/dev/null 2>&1 || status=$?
  exit "$status"
}
trap live_runner_cleanup HUP INT TERM EXIT
live_event run_started "$(jq -cn --arg profile "$profile" '{profile:$profile}')"
if $legacy; then
  live_run_bounded "$(jq -r --arg p "$profile" '.profiles[$p].wall_seconds' "$LIVE_PROJECT_ROOT/tests/live/contracts/profiles.json")" \
    "$LIVE_RUN_ROOT/artifacts/legacy.stdout" "$LIVE_RUN_ROOT/artifacts/legacy.stderr" "$legacy_runner" "${legacy_args[@]}" && result=pass || result=fail
  live_event legacy_result "$(jq -cn --arg result "$result" --arg stdout artifacts/legacy.stdout --arg stderr artifacts/legacy.stderr '{schema:"cortex-live-legacy-result-v1",isolated_from_capability_ledger:true,result:$result,stdout:$stdout,stderr:$stderr}')"
fi
live_budget_check "$profile" "$LIVE_PROJECT_ROOT/tests/live/contracts/profiles.json"
live_report
live_secret_scan "$LIVE_RUN_ROOT"
live_run_manifest_verify
live_ledger_validate "$LIVE_SURFACE_CONTRACT" "$profile"
jq -e '.failed == 0' "$LIVE_RUN_ROOT/summary.json" >/dev/null || live_die "one or more live scenarios failed"
