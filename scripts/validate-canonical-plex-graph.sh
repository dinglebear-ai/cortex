#!/usr/bin/env bash
# Read-only proof workflow for the canonical entity-resolution graph
# contract (entity_resolution_v2), using Plex as the worked example.
#
# Reports:
#   - old_key_count: legacy service identity rows ('nashost:plex',
#     'nashost:plex:plex', nested 'plex/plex/plex' app labels). Must be 0
#     after migration 41 + a resolver rebuild.
#   - new_key_count: canonical rows ('logical_service:plex',
#     'service_instance:nashost/plex' / 'backuphost/plex'). Must be > 0 once
#     resolver projection has seen agent-docker or inventory evidence.
#   - the query plan for the canonical lookup (index-backed, no scan).
#
# This script is read-only by default and REFUSES any live rebuild. To
# rebuild for real: take a WAL-safe backup first (`cortex db backup`), run
# `cortex graph rebuild` off-peak with a timeout, then re-run this script
# and assert old_key_count=0.
set -euo pipefail

db_path="${CORTEX_DB_PATH:-data/cortex.db}"
mode="${1:-read-only}"

if [ "$mode" != "read-only" ]; then
  echo "Refusing live rebuild in validation script. Run read-only checks first, create a WAL-safe backup, and use documented operator commands for rebuild." >&2
  exit 2
fi

if [ ! -f "$db_path" ]; then
  echo "Cortex DB not found: $db_path" >&2
  exit 1
fi

old_count="$(sqlite3 -readonly "$db_path" "
SELECT COUNT(*)
  FROM graph_entities
 WHERE (entity_type = 'service' AND canonical_key IN ('nashost:plex', 'nashost:plex:plex'))
    OR (entity_type = 'app' AND canonical_key = 'plex/plex/plex');
")"
echo "old_key_count=$old_count"

new_count="$(sqlite3 -readonly "$db_path" "
SELECT COUNT(*)
  FROM graph_entities
 WHERE (entity_type = 'logical_service' AND canonical_key = 'plex')
    OR (entity_type = 'service_instance' AND canonical_key IN ('nashost/plex', 'backuphost/plex'));
")"
echo "new_key_count=$new_count"

sqlite3 -readonly "$db_path" "
EXPLAIN QUERY PLAN
SELECT id, entity_type, canonical_key
  FROM graph_entities
 WHERE entity_type IN ('logical_service', 'service_instance')
   AND canonical_key IN ('plex', 'nashost/plex');
"

echo "Read-only validation complete. old_key_count must be 0 after rebuild; new_key_count must be greater than 0 after resolver projection."
