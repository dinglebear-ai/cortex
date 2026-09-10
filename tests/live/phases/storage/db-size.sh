#!/usr/bin/env bash
set -euo pipefail
: "${LIVE_RUN_ROOT:?}" "${LIVE_COMPOSE_PROJECT:?}" "${LIVE_SYSLOG_TCP_PORT:?}"
root="${LIVE_PROJECT_ROOT:?}"; base="$root/tests/live/profiles/isolated/compose.yaml"; override="$root/tests/live/profiles/storage/compose.override.yaml"; budget="$root/tests/live/profiles/storage/pressure-budget.override.yaml"
# shellcheck disable=SC1091
source "$root/tests/live/lib/common.sh"; source "$root/tests/live/lib/lock.sh"; source "$root/tests/live/lib/redact.sh"; source "$root/tests/live/lib/events.sh"; source "$root/tests/live/lib/budgets.sh"; source "$root/tests/live/lib/wait.sh"; source "$root/tests/live/lib/docker.sh"; source "$root/tests/live/lib/storage_diag.sh"
mkdir -p "$LIVE_RUN_ROOT/artifacts/storage"
# Trim under a budget whose recovery target sits above the shared volume's
# non-log floor; see pressure-budget.override.yaml.
docker compose -f "$base" -f "$override" -f "$budget" -p "$LIVE_COMPOSE_PROJECT" up -d --no-deps --no-build --force-recreate candidate >/dev/null
live_wait_until 60 db-size-health _live_http_health_ready
candidate="$(docker compose -f "$base" -f "$override" -f "$budget" -p "$LIVE_COMPOSE_PROJECT" ps -q candidate)"
state="$(docker volume ls -q --filter "label=com.docker.compose.project=$LIVE_COMPOSE_PROJECT" --filter label=cortex.live.kind=state)"
fixture="$LIVE_RUN_ROOT/artifacts/storage/db-size-fixture.syslog"
padding="$(awk 'BEGIN{for(i=0;i<6900;i++)printf "x"}')"
: >"$fixture"
for index in $(seq 1 200); do printf '<134>1 2026-08-27T00:00:00Z cortex-live pressure - - - db-size-%04d %s\n' "$index" "$padding" >>"$fixture"; done
for index in $(seq 1 715); do printf '<131>1 2026-08-27T00:00:00Z cortex-live pressure - - - db-size-error-%04d %s\n' "$index" "$padding" >>"$fixture"; done
fixture_bytes="$(wc -c <"$fixture" | tr -d ' ')"; live_fixture_account 915 "$fixture_bytes"; live_connection_opened 1
sent="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
nc -w 30 127.0.0.1 "$LIVE_SYSLOG_TCP_PORT" <"$fixture"
live_wait_until 60 db-size-ingest _live_ingest_ready db-size-error-0715
status="$LIVE_RUN_ROOT/artifacts/storage/db-size-status.json"
ticks="$LIVE_RUN_ROOT/artifacts/storage/db-size-ticks.log"; recovery_tick="$LIVE_RUN_ROOT/artifacts/storage/db-size-recovery-tick.log"
# Recovery is proven by the product's own report, not by sampling the size:
# ingest continues after a trim, and between the 8 MiB target and the 12 MiB
# trigger the database is in budget and correctly left alone, so a later
# sample can sit above 8 MiB with nothing wrong. Require an enforcement tick,
# logged after the fixture was sent, that trimmed rows and finished at or
# under the recovery target with writes flowing.
_db_size_trim_completed() {
  docker logs --since "$sent" "$candidate" 2>&1 | grep -F 'Storage budget enforcement tick completed' >"$ticks" || return 1
  awk '{ d = ""; l = ""; b = ""
         for (i = 1; i <= NF; i++) {
           if ($i ~ /^deleted_rows=/) d = substr($i, 14)
           else if ($i ~ /^logical_db_size_bytes=/) l = substr($i, 23)
           else if ($i ~ /^write_blocked=/) b = substr($i, 15)
         }
         if (d + 0 > 0 && l != "" && l + 0 <= 8388608 && b == "false") { print; found = 1; exit }
       }
       END { exit !found }' "$ticks" >"$recovery_tick"
}
live_wait_until 120 db-size-recovery _db_size_trim_completed || { rc=$?; live_storage_diagnose db-size-recovery "$state" "$candidate"; exit "$rc"; }
recovered_bytes="$(grep -oE 'logical_db_size_bytes=[0-9]+' "$recovery_tick" | cut -d= -f2)"
docker compose -f "$base" -f "$override" -p "$LIVE_COMPOSE_PROJECT" exec -T -e RUST_LOG=error candidate cortex db status --json >"$status"
errors="$LIVE_RUN_ROOT/artifacts/storage/db-size-errors.json"
docker compose -f "$base" -f "$override" -p "$LIVE_COMPOSE_PROJECT" exec -T -e RUST_LOG=error candidate cortex search --grep db-size-error --limit 100 --json >"$errors"
count="$(jq -r .count "$errors")"; (( count >= 10 && count < 715 )) || live_die "err-floor pressure semantics not observed: $count"
jq -cn --argjson fixture_bytes "$fixture_bytes" --argjson logical_size_bytes "$recovered_bytes" --argjson current_bytes "$(jq -r .logical_size_bytes "$status")" --argjson protected_errors "$count" \
  '{schema:"cortex-live-db-size-pressure-v1",disposition:"pass",fixture_bytes:$fixture_bytes,recovered_below_bytes:8388608,logical_size_bytes:$logical_size_bytes,current_logical_size_bytes:$current_bytes,error_floor_per_source_cap:10,remaining_errors:$protected_errors,observed_excess_errors_deleted:($protected_errors<715),observed_floor_minimum_preserved:($protected_errors>=10)}' \
  >"$LIVE_RUN_ROOT/artifacts/storage/db-size.json"
chmod 600 "$LIVE_RUN_ROOT/artifacts/storage/db-size"*.json "$ticks" "$recovery_tick"
