#!/usr/bin/env bash
set -euo pipefail

# Resolve /backups through Compose so .env interpolation and CLI overrides use
# exactly the same parser as the subsequent `docker compose up`.
compose_json="$(docker compose "$@" config --format json)"
backup_dir="$(python3 -c '
import json
import os
import sys

project = json.load(sys.stdin)
service = project.get("services", {}).get("cortex", {})
for volume in service.get("volumes", []):
    if volume.get("target") == "/backups":
        if volume.get("type") != "bind":
            raise SystemExit("cortex /backups mount must be a bind mount")
        source = volume.get("source", "")
        if not os.path.isabs(source):
            raise SystemExit("cortex /backups bind source must resolve to an absolute path")
        print(source)
        break
else:
    raise SystemExit("cortex Compose service has no /backups mount")
  ' <<<"$compose_json"
)"

install -d -m 700 "$backup_dir"
[[ "$(stat -c '%a' "$backup_dir" 2>/dev/null || stat -f '%Lp' "$backup_dir")" == "700" ]]
printf 'Prepared Cortex backup directory: %s (0700)\n' "$backup_dir"
