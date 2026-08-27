#!/usr/bin/env bash
set -euo pipefail
LIVE_PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"; export LIVE_PROJECT_ROOT
# shellcheck disable=SC1090
for lib in common lock redact events command lease resources report artifacts contracts budgets wait diagnostics docker; do source "$LIVE_PROJECT_ROOT/tests/live/lib/$lib.sh"; done

usage() { echo "usage: tests/live/runner.sh [--profile noop|smoke|full|soak|isolated|docker-boundary-reduced|docker-boundary-full] [--candidate-image IMAGE --oracle-image IMAGE --toxiproxy-image IMAGE] [--runs-root DIR] [--janitor] [--provider ID] [--target ID] [--legacy ARGS...]"; }
profile=smoke; boundary_status=0; runs_root="${TMPDIR:-/tmp}/cortex-live-runs"; janitor=false; provider="local:$LIVE_PROJECT_ROOT"; target="local"; legacy=false; legacy_args=(); legacy_runner="${LIVE_LEGACY_RUNNER:-$LIVE_PROJECT_ROOT/tests/test_live.sh}"; candidate_image="${LIVE_CANDIDATE_IMAGE_REF:-}"; oracle_image="${LIVE_ORACLE_IMAGE_REF:-}"; toxiproxy_image="${LIVE_TOXIPROXY_IMAGE_REF:-}"
while (($#)); do case "$1" in --profile) profile="$2"; shift 2;; --candidate-image) candidate_image="$2"; shift 2;; --oracle-image) oracle_image="$2"; shift 2;; --toxiproxy-image) toxiproxy_image="$2"; shift 2;; --runs-root) runs_root="$2"; shift 2;; --provider) provider="$2"; shift 2;; --target) target="$2"; shift 2;; --janitor) janitor=true; shift;; --legacy) legacy=true; shift; legacy_args=("$@"); break;; -h|--help) usage; exit;; *) usage >&2; exit 2;; esac; done
live_require_tools bash jq openssl cargo shasum ps pgrep find stat sed awk || live_die "live harness prerequisites unavailable"
jq -e --arg p "$profile" '.profiles[$p]' "$LIVE_PROJECT_ROOT/tests/live/contracts/profiles.json" >/dev/null || live_die "unknown profile: $profile"
if $janitor; then live_janitor "$runs_root" "$provider"; exit; fi
live_init_run "$runs_root" >/dev/null
live_contract_export "$LIVE_RUN_ROOT/surface-contract.json"
live_run_manifest_write "$profile" "$provider" "$target" "$LIVE_SURFACE_CONTRACT"
live_budget_start
live_lease_write 120
live_runner_cleanup() {
  local status=$? cleanup_provider="${LIVE_RESOURCE_PROVIDER:-$provider}" resource_file="$LIVE_RUN_ROOT/resources.jsonl"
  trap - HUP INT TERM EXIT
  if [[ -f "$resource_file" ]] && jq -e -s 'length>0 and ([.[].provider]|unique|length)==1' "$resource_file" >/dev/null 2>&1; then
    cleanup_provider="$(jq -sr '.[0].provider' "$resource_file")"
  fi
  if [[ "$cleanup_provider" == docker-host:* ]]; then
    current_docker_id="$(docker info --format '{{.ID}}' 2>/dev/null || true)"
    [[ "docker-host:$current_docker_id" == "$cleanup_provider" ]] || cleanup_provider="docker-host:identity-mismatch"
  fi
  live_cleanup_resources "$cleanup_provider" >/dev/null 2>&1 || status=$?
  exit "$status"
}
trap live_runner_cleanup HUP INT TERM EXIT
live_event run_started "$(jq -cn --arg profile "$profile" '{profile:$profile}')"
case "$profile" in
  isolated)
    [[ -n "$candidate_image" && -n "$oracle_image" && -n "$toxiproxy_image" ]] || live_die "isolated profile requires three explicit image references"
    live_topology_start "$candidate_image" "$oracle_image" "$toxiproxy_image"
    if [[ "${LIVE_ISOLATED_HOLD_SECONDS:-0}" =~ ^[1-9][0-9]*$ ]]; then
      hold_until=$(( $(date +%s) + LIVE_ISOLATED_HOLD_SECONDS ))
      while (( $(date +%s) < hold_until )); do sleep 1; done
    fi
    ;;
  docker-boundary-reduced)
    set +e
    # shellcheck disable=SC1091
    CORTEX_LIVE_DOCKER_BOUNDARY_MODE=reduced source "$LIVE_PROJECT_ROOT/tests/live/spikes/docker-boundary/scenario.sh"
    boundary_status=$?; set -e
    ;;
  docker-boundary-full)
    set +e
    # shellcheck disable=SC1091
    CORTEX_LIVE_DOCKER_BOUNDARY_MODE=full source "$LIVE_PROJECT_ROOT/tests/live/spikes/docker-boundary/scenario.sh"
    boundary_status=$?; set -e
    ;;
esac
if [[ "$profile" == docker-boundary-* ]]; then
  boundary_evidence="$LIVE_RUN_ROOT/artifacts/docker-boundary.json"
  if [[ ! -f "$boundary_evidence" ]] || ! jq -e '.schema=="cortex-live-docker-boundary-result-v1" and .disposition=="pass"' "$boundary_evidence" >/dev/null; then boundary_status=1; fi
fi
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
live_summary_accepts_profile "$profile" "$LIVE_PROJECT_ROOT/tests/live/contracts/profiles.json" "$LIVE_RUN_ROOT/summary.json" || live_die "mandatory profile failed or produced a qualified/non-green outcome"
(( boundary_status == 0 )) || live_die "mandatory Docker boundary scenario did not pass"
