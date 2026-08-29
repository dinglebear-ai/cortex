#!/usr/bin/env bash
set -euo pipefail

image_ref=${1:?usage: test-backup-contract.sh IMAGE_REF}
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT
source_dir="$test_root/data"
backup_dir="$test_root/backups"
mkdir -p "$source_dir" "$backup_dir"

file_mode() {
  stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"
}

docker run --rm --user 0:0 -v "$source_dir:/data" "$image_ref" sqlite3 /data/cortex.db \
  "CREATE TABLE proof(value TEXT); INSERT INTO proof VALUES('syslog-survives');"
docker run --rm --user 0:0 -v "$source_dir:/data" "$image_ref" sqlite3 /data/auth.db \
  "CREATE TABLE proof(value TEXT); INSERT INTO proof VALUES('auth-survives');"
printf '%s\n' 'test-signing-key' >"$source_dir/auth-jwt.pem"

docker run --rm --user 0:0 -e CORTEX_DB_PATH=/data/cortex.db \
  -v "$source_dir:/data" -v "$backup_dir:/backups" "$image_ref" \
  bash /usr/local/libexec/cortex-backup.sh /backups

syslog_backup="$(find "$backup_dir" -name 'syslog-*.db' -print -quit)"
auth_backup="$(find "$backup_dir" -name 'auth-*.db' -print -quit)"
key_backup="$(find "$backup_dir" -name 'auth-jwt-*.pem' -print -quit)"
[[ -n "$syslog_backup" && -n "$auth_backup" && -n "$key_backup" ]]
[[ "$(file_mode "$backup_dir")" == "700" ]]
for artifact in "$syslog_backup" "$auth_backup" "$key_backup"; do
  [[ "$(file_mode "$artifact")" == "600" ]]
done

# Model loss of the data volume. Backups remain usable on their independent bind.
rm -rf "$source_dir"
[[ "$(docker run --rm -v "$backup_dir:/backups:ro" "$image_ref" sqlite3 "/backups/$(basename "$syslog_backup")" 'SELECT value FROM proof;')" == "syslog-survives" ]]
[[ "$(docker run --rm -v "$backup_dir:/backups:ro" "$image_ref" sqlite3 "/backups/$(basename "$auth_backup")" 'SELECT value FROM proof;')" == "auth-survives" ]]
[[ "$(cat "$key_backup")" == "test-signing-key" ]]

echo "Backup contents, isolation, recovery, and permission contract passed"
