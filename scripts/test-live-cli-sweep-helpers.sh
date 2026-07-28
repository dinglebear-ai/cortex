#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT/scripts/live-cli-sweep-helpers.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

mkdir -p "$TMP/bin"
cat >"$TMP/bin/cortex" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == "--version" ]]; then
  echo "cortex ${MOCK_HOST_VERSION:-3.6.5}"
  exit 0
fi
if [[ "$*" == db\ integrity\ status* ]]; then
  count_file="${MOCK_STATUS_COUNT_FILE:?}"
  count=0
  [[ -f "$count_file" ]] && count="$(cat "$count_file")"
  count=$((count + 1))
  printf '%s\n' "$count" >"$count_file"
  case "${MOCK_JOB_MODE:-done}" in
    done) printf '{"job_id":7,"status":"done","integrity":{"ok":true,"messages":["ok"]}}\n' ;;
    failed) printf '{"job_id":7,"status":"failed","error":"disk read failed"}\n' ;;
    corrupt) printf '{"job_id":7,"status":"done","integrity":{"ok":false,"messages":["bad"]}}\n' ;;
    stuck) printf '{"job_id":7,"status":"running"}\n' ;;
  esac
  exit 0
fi
exit 2
EOF
chmod +x "$TMP/bin/cortex"

cat >"$TMP/bin/curl" <<'EOF'
#!/usr/bin/env bash
printf '{"version":"%s"}\n' "${MOCK_API_VERSION:-3.6.5}"
EOF
chmod +x "$TMP/bin/curl"

cat >"$TMP/runtime-check" <<'EOF'
#!/usr/bin/env bash
if [[ "${MOCK_RUNTIME_OK:-true}" != "true" ]]; then
  echo "STALE: running container image differs from local compose image"
  exit 1
fi
cat <<OUT
mode       docker
image      ghcr.io/dinglebear-ai/cortex:3.6.5
running_image_id sha256:current
local_image_id sha256:current
repo_version 3.6.5
container_version 3.6.5
CURRENT: running container matches local compose image and repo version
OUT
EOF
chmod +x "$TMP/runtime-check"

export PATH="$TMP/bin:$PATH"
export CORTEX_SWEEP_CORTEX_BIN="$TMP/bin/cortex"
export CORTEX_SWEEP_RUNTIME_CHECK="$TMP/runtime-check"
export CORTEX_API_TOKEN=test-token
export MOCK_STATUS_COUNT_FILE="$TMP/status-count"

out="$($HELPER preflight "$ROOT" "http://127.0.0.1:3100")"
[[ "$out" == *"runtime parity ok: host=3.6.5 api=3.6.5 container=3.6.5"* ]] \
  || fail "preflight success output: $out"

set +e
out="$(MOCK_API_VERSION=3.6.4 $HELPER preflight "$ROOT" "http://127.0.0.1:3100" 2>&1)"
status=$?
set -e
[[ $status -ne 0 && "$out" == *"API version 3.6.4 does not match host version 3.6.5"* ]] \
  || fail "API mismatch status=$status output=$out"

set +e
out="$(MOCK_RUNTIME_OK=false $HELPER preflight "$ROOT" "http://127.0.0.1:3100" 2>&1)"
status=$?
set -e
[[ $status -ne 0 && "$out" == *"running container image differs"* ]] \
  || fail "image mismatch status=$status output=$out"

rm -f "$MOCK_STATUS_COUNT_FILE"
out="$(MOCK_JOB_MODE=done $HELPER wait-integrity '{"job_id":7,"status":"running"}' 2 0)"
[[ "$out" == *'"status":"done"'* && "$out" == *'"ok":true'* ]] \
  || fail "done integrity output: $out"

set +e
out="$(MOCK_JOB_MODE=failed $HELPER wait-integrity '{"job_id":7,"status":"running"}' 2 0 2>&1)"
status=$?
set -e
[[ $status -ne 0 && "$out" == *"integrity job 7 failed: disk read failed"* ]] \
  || fail "failed integrity status=$status output=$out"

set +e
out="$(MOCK_JOB_MODE=corrupt $HELPER wait-integrity '{"job_id":7,"status":"running"}' 2 0 2>&1)"
status=$?
set -e
[[ $status -ne 0 && "$out" == *"completed without an ok integrity result"* ]] \
  || fail "corrupt integrity status=$status output=$out"

set +e
out="$(MOCK_JOB_MODE=stuck $HELPER wait-integrity '{"job_id":7,"status":"running"}' 0 0 2>&1)"
status=$?
set -e
[[ $status -ne 0 && "$out" == *"timed out waiting for integrity job 7"* ]] \
  || fail "stuck integrity status=$status output=$out"

echo "live CLI sweep helper tests passed"
