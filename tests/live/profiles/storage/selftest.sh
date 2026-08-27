#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
fail() { echo "storage selftest: $*" >&2; exit 1; }
for f in "$root/tests/live/phases/storage/run.sh" "$root/tests/live/phases/storage/retention.sh" "$root/tests/live/phases/storage/db-size.sh" "$root/tests/live/phases/storage/cleanup-faults.sh" "$root/tests/live/phases/storage/pressure.sh" "$root/tests/live/phases/lifecycle/run.sh" "$root/tests/live/profiles/storage/pressure-watchdog.sh" "$root/tests/live/profiles/storage/quota-resource.sh"; do bash -n "$f"; done
jq -e '.pressure.requires_verified_hard_quota and .pressure.external_watchdog and .safety.deployed_targets_forbidden and .database.backup_invariants==["wal-safe","integrity-ok","marker-readable"]' "$root/tests/live/contracts/storage.json" >/dev/null
grep -q 'platform-qualified' "$root/tests/live/phases/storage/pressure.sh" || fail "missing honest no-quota disposition"
grep -q 'CORTEX_LIVE_STORAGE_PRESSURE_AUTHORIZED' "$root/tests/live/phases/storage/pressure.sh" || fail "pressure is not authorization gated"
grep -q 'max_written_bytes' "$root/tests/live/phases/storage/pressure.sh" || fail "missing byte cap"
grep -q 'pressure-watchdog.sh' "$root/tests/live/phases/storage/pressure.sh" || fail "missing external watchdog"
grep -Fq "\"\$\$\" \"\$deadline\"" "$root/tests/live/phases/storage/pressure.sh" || fail "watchdog does not own the complete pressure phase"
! grep -Fq "pressure-watchdog.sh\" \"\$filler\"" "$root/tests/live/phases/storage/pressure.sh" || fail "watchdog only covers filler subprocess"
grep -q 'storage-quota-volume.*PLANNED' "$root/tests/live/phases/storage/pressure.sh" || fail "quota volume is not registered before creation"
grep -q 'storage-quota-container.*PLANNED' "$root/tests/live/phases/storage/pressure.sh" || fail "quota container is not registered before creation"
grep -q 'docker create --name' "$root/tests/live/phases/storage/pressure.sh" || fail "quota container identity is not reconciled before start"
grep -q 'watchdog cannot read the quota volume byte-accounting path' "$root/tests/live/phases/storage/pressure.sh" || fail "watchdog byte accounting can silently degrade"
grep -q 'chmod 0755.*fixtures' "$root/tests/live/phases/storage/pressure.sh" || fail "UID 1000 fixture traversal is not portable"
grep -q 'profile lacks mandatory otlp-storage-blocked evidence' "$root/tests/live/runner.sh" || fail "storage/full final reconciliation is absent"
awk '/  full\)/,/    ;;/ {print}' "$root/tests/live/runner.sh" | grep -q 'phases/storage/pressure.sh' || fail "full profile does not execute storage pressure obligation"
# Exercise the watchdog as a process supervisor, not merely as shell syntax.
watch_tmp="$(mktemp -d)"; trap 'rm -rf "$watch_tmp"' EXIT
sleep 1 & watched=$!
"$root/tests/live/profiles/storage/pressure-watchdog.sh" "$watched" "$(( $(date +%s)+10 ))" 1048576 "$watch_tmp"
wait "$watched"
sleep 30 & watched=$!
set +e
"$root/tests/live/profiles/storage/pressure-watchdog.sh" "$watched" "$(( $(date +%s)+1 ))" 1048576 "$watch_tmp"
watch_status=$?
set -e
[[ "$watch_status" == 124 ]] || fail "watchdog did not terminate a phase at its external deadline"
! kill -0 "$watched" 2>/dev/null || fail "deadline watchdog left its supervised process alive"
wait "$watched" 2>/dev/null || true
rm -rf "$watch_tmp"; trap - EXIT
grep -q 'metric_storage_blocked' "$root/tests/live/phases/storage/pressure.sh" || fail "missing compiled OTLP metric backpressure assertion"
grep -q 'rejected_spans' "$root/tests/live/phases/storage/pressure.sh" || fail "missing compiled OTLP trace partial-success assertion"
grep -q 'otlp-storage-blocked' "$root/tests/live/phases/storage/pressure.sh" || fail "missing cross-bead evidence event"
grep -q 'MetricStorageBlocked.into_response' "$root/src/otlp/metric_http.rs" || fail "OTLP metrics production semantics drifted"
grep -q 'return trace_success_response(rejected' "$root/src/otlp/trace_http.rs" || fail "OTLP traces production semantics drifted"
grep -q 'retaining batch until space recovers' "$root/src/receiver/writer.rs" || fail "OTLP log retention semantics drifted"
grep -q -- '--force-recreate candidate' "$root/tests/live/phases/lifecycle/run.sh" || fail "missing replacement test"
grep -q 'CORTEX_DB_PATH=/data/live-concurrent-backup.db' "$root/tests/live/phases/storage/run.sh" || fail "backup is not independently opened"
grep -q 'storage-restore-volume.*PLANNED' "$root/tests/live/phases/storage/run.sh" || fail "restore volume is not registered before creation"
grep -q -- '--user 1000:1000' "$root/tests/live/phases/storage/run.sh" || fail "restore verification does not run as the production UID"
grep -q 'chown 1000:1000 /data/cortex.db /data/committed-markers.txt' "$root/tests/live/phases/storage/run.sh" || fail "restore evidence ownership is not explicit"
grep -q 'stat -c %u:%g:%a /data/committed-markers.txt' "$root/tests/live/phases/storage/run.sh" || fail "restore marker permissions are not asserted live"
! grep -Fq -- "-v \"\$markers:/run-markers:ro\"" "$root/tests/live/phases/storage/run.sh" || fail "UID1000 verifier still depends on the host marker bind"
grep -q 'host_heartbeats' "$root/tests/live/phases/storage/retention.sh" || fail "heartbeat cap is not tested"
grep -q 'phantom_fts_rows' "$root/tests/live/phases/storage/retention.sh" || fail "FTS phantom behavior is not observed"
grep -q 'error_floor_per_source_cap' "$root/tests/live/phases/storage/db-size.sh" || fail "DB pressure does not verify the err floor cap"
! grep -R -E 'docker compose down|systemctl' "$root/tests/live/phases/storage" "$root/tests/live/phases/lifecycle" >/dev/null || fail "phase can target deployed storage"
echo 'storage selftest: PASS'
