#!/usr/bin/env bash
set -euo pipefail
: "${LIVE_RUN_ROOT:?}" "${LIVE_COMPOSE_PROJECT:?}" "${LIVE_ORACLE_IMAGE:?}"
root="${LIVE_PROJECT_ROOT:?}"; base="$root/tests/live/profiles/isolated/compose.yaml"; override="$root/tests/live/profiles/storage/compose.override.yaml"; fault_override="$root/tests/live/profiles/storage/cleanup-fault.override.yaml"; budget="$root/tests/live/profiles/storage/pressure-budget.override.yaml"
# shellcheck disable=SC1091
source "$root/tests/live/lib/common.sh"; source "$root/tests/live/lib/lock.sh"; source "$root/tests/live/lib/redact.sh"; source "$root/tests/live/lib/events.sh"; source "$root/tests/live/lib/budgets.sh"; source "$root/tests/live/lib/wait.sh"; source "$root/tests/live/lib/docker.sh"; source "$root/tests/live/lib/storage_diag.sh"
live_install_err_trap
mkdir -p "$LIVE_RUN_ROOT/artifacts/storage"
state="$(docker volume ls -q --filter "label=com.docker.compose.project=$LIVE_COMPOSE_PROJECT" --filter label=cortex.live.kind=state)"
fixture="$LIVE_RUN_ROOT/artifacts/storage/db-size-fixture.syslog"
docker compose -f "$base" -f "$override" -f "$budget" -f "$fault_override" -p "$LIVE_COMPOSE_PROJECT" up -d --no-build --force-recreate candidate >/dev/null
live_wait_until 60 cleanup-fault-health _live_http_health_ready || { rc=$?; live_storage_diagnose cleanup-fault-health "$state" "$candidate"; exit "$rc"; }
candidate="$(docker compose -f "$base" -f "$override" -f "$fault_override" -p "$LIVE_COMPOSE_PROJECT" ps -q candidate)"

# Refill well above the 12 MiB trigger (pressure-budget.override.yaml), then hold an external SQLite write lock across
# a cleanup tick. The failure must be visible and the following tick recover.
{ cat "$fixture"; cat "$fixture"; } | nc -w 30 127.0.0.1 "$LIVE_SYSLOG_TCP_PORT"; live_connection_opened 1
# nc only proves the socket accepted the bytes. Wait until the batch writer has
# committed enough data to cross the configured 12 MiB enforcement threshold;
# otherwise the external lock can precede the write and cleanup correctly has
# no over-budget work to fail.
_cleanup_pressure_ready() { docker compose -f "$base" -f "$override" -p "$LIVE_COMPOSE_PROJECT" exec -T -e RUST_LOG=error candidate cortex db status --json 2>/dev/null | jq -e '.logical_size_bytes>12582912' >/dev/null; }
live_wait_until 60 cleanup-pressure-ready _cleanup_pressure_ready || { rc=$?; live_storage_diagnose cleanup-pressure-ready "$state" "$candidate"; exit "$rc"; }
lock_ev="$LIVE_RUN_ROOT/artifacts/storage/cleanup-lock.txt"
docker run --rm --user 0:0 -v "$state:/data" --entrypoint python "$LIVE_ORACLE_IMAGE" -c '
import sqlite3,time
db=sqlite3.connect("/data/cortex.db",timeout=30); db.execute("BEGIN EXCLUSIVE"); print("LOCKED",flush=True); time.sleep(45); db.rollback(); db.close()
' >"$lock_ev" 2>&1 & locker=$!
live_wait_until 10 cleanup-lock-acquired grep -q LOCKED "$lock_ev"
# SQLite connections use a five-second busy timeout. A synchronized 45-second
# hold across the fault profile's 30-second cadence guarantees the next tick reaches its
# busy timeout while the external lock is still owned, even under CI jitter.
sleep 41; wait "$locker"
docker logs "$candidate" 2>&1 | grep -F 'Failed to enforce storage budget' >"$LIVE_RUN_ROOT/artifacts/storage/cleanup-failure.log" || live_die "cleanup failure was not observed"
since="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
docker compose -f "$base" -f "$override" -f "$budget" -p "$LIVE_COMPOSE_PROJECT" up -d --no-build --force-recreate candidate >/dev/null
live_wait_until 60 cleanup-recovery-health _live_http_health_ready || { rc=$?; live_storage_diagnose cleanup-recovery-health "$state" "$candidate"; exit "$rc"; }
candidate="$(docker compose -f "$base" -f "$override" -p "$LIVE_COMPOSE_PROJECT" ps -q candidate)"
# A replacement process's startup storage check is the product's own decision
# point: over the 12 MiB trigger it trims to <= 8 MiB before serving; within
# budget it correctly leaves the database alone. The 8-12 MiB hysteresis band
# is not durable across a restart, so "<= 8 MiB after any restart" is not a
# guarantee the product makes. Hold each replacement to what it decided.
_cleanup_startup_check() {
  local label="$1" since="$2" line rc
  _cleanup_startup_logged() { docker logs --since "$since" "$candidate" 2>&1 | grep -F 'Initial storage budget check completed' >/dev/null; }
  live_wait_until 60 "$label-startup-check" _cleanup_startup_logged || { rc=$?; live_storage_diagnose "$label-startup-check" "$state" "$candidate" "${marker:-}"; exit "$rc"; }
  line="$(docker logs --since "$since" "$candidate" 2>&1 | grep -F 'Initial storage budget check completed' | tail -1)"
  startup_deleted="$(grep -oE 'deleted_rows=[0-9]+' <<<"$line" | cut -d= -f2)"
  startup_logical="$(grep -oE 'logical_db_size_bytes=[0-9]+' <<<"$line" | cut -d= -f2)"
  [[ "$startup_deleted" =~ ^[0-9]+$ && "$startup_logical" =~ ^[0-9]+$ && "$line" == *write_blocked=false* ]] || { live_die "$label: unreadable or write-blocked startup storage check: $line"; return 1; }
  if (( startup_deleted > 0 )); then
    (( startup_logical <= 8388608 )) || { live_die "$label: replacement trimmed $startup_deleted rows but stopped at $startup_logical bytes, above the 8 MiB recovery target"; return 1; }
  else
    (( startup_logical <= 12582912 )) || { live_die "$label: replacement started at $startup_logical bytes, over the 12 MiB trigger, without trimming"; return 1; }
  fi
}
_cleanup_startup_check cleanup-failure-recovery "$since"

# Refill once more and restart as soon as the one-row cleanup loop begins. The
# replacement must resume or correctly skip cleanup, preserve the newest
# marker, and remain sound.
refilled="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
marker="cleanup-interrupt-${LIVE_RUN_ID#cortex-e2e-}"; { cat "$fixture"; cat "$fixture"; printf '<134>1 %s cortex-live cleanup - - - %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$marker"; } | nc -w 30 127.0.0.1 "$LIVE_SYSLOG_TCP_PORT"; live_connection_opened 1
# The marker lands behind two copies of the pressure fixture while one-row
# cleanup trims under the storage budget; give it the same bound as the
# recovery wait above, not a 30 s window hosted runners cannot meet.
live_wait_until 120 cleanup-interrupt-marker _live_ingest_ready "$marker" || { rc=$?; live_storage_diagnose cleanup-interrupt-marker "$state" "$candidate" "$marker"; exit "$rc"; }
_cleanup_started() { docker logs --since "$refilled" "$candidate" 2>&1 | grep -F 'self-trimming oldest telemetry chunk' >/dev/null; }
live_wait_until 30 cleanup-started _cleanup_started || { rc=$?; live_storage_diagnose cleanup-started "$state" "$candidate" "$marker"; exit "$rc"; }
since="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
docker compose -f "$base" -f "$override" -p "$LIVE_COMPOSE_PROJECT" restart candidate >/dev/null
live_wait_until 60 cleanup-restart-health _live_http_health_ready || { rc=$?; live_storage_diagnose cleanup-restart-health "$state" "$candidate" "$marker"; exit "$rc"; }
_cleanup_startup_check cleanup-restart "$since"
if (( startup_deleted > 0 )); then interrupted=true; else interrupted=false; fi
_live_ingest_ready "$marker" || live_die "newest committed marker lost across interrupted cleanup"
docker compose -f "$base" -f "$override" -p "$LIVE_COMPOSE_PROJECT" exec -T -e RUST_LOG=error candidate cortex db integrity --quick --json >"$LIVE_RUN_ROOT/artifacts/storage/cleanup-recovery-integrity.json"
jq -cn --argjson interrupted "$interrupted" --argjson deleted "$startup_deleted" --argjson logical "$startup_logical" '{schema:"cortex-live-cleanup-faults-v1",cleanup_failure_observed:true,failure_recovered:true,cleanup_interrupted_by_restart:$interrupted,restart_startup_deleted_rows:$deleted,restart_startup_logical_bytes:$logical,restart_recovered:true,newest_marker_preserved:true,integrity_ok:true}' >"$LIVE_RUN_ROOT/artifacts/storage/cleanup-faults.json"
chmod 600 "$LIVE_RUN_ROOT/artifacts/storage/cleanup-"* "$LIVE_RUN_ROOT/artifacts/storage/cleanup-faults.json"
