#!/usr/bin/env sh
# Offline restore only: stop Cortex before running this helper.
set -eu
backup_dir=${1:?backup directory required}
stamp=${2:?backup timestamp required}
data_dir=${3:?data directory required}
case "$stamp" in *[!A-Za-z0-9_.-]*|'') echo 'invalid backup timestamp' >&2; exit 64;; esac
umask 077
stage=$(mktemp -d "$data_dir/.restore.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
# Every copy must finish before any current database or recovery sidecar changes.
cp "$backup_dir/syslog-$stamp.db" "$stage/cortex.db"
for entry in "auth-$stamp.db:auth.db" "auth-jwt-$stamp.pem:auth-jwt.pem" "integration-credential-$stamp.key:integration-credential.key"; do
  source=${entry%:*}; target=${entry#*:}
  if [ -e "$backup_dir/$source" ]; then
    cp "$backup_dir/$source" "$stage/$target"
  fi
done
for target in cortex.db auth.db; do
  if [ -f "$stage/$target" ]; then
    test -s "$stage/$target"
    result=$(sqlite3 -readonly "$stage/$target" 'PRAGMA integrity_check;')
    if [ "$result" != ok ]; then
      echo "backup integrity check failed: $target" >&2
      exit 1
    fi
  fi
done
for target in cortex.db auth.db auth-jwt.pem integration-credential.key; do
  if [ -f "$stage/$target" ]; then
    mv -f "$stage/$target" "$data_dir/$target"
    case "$target" in *.db) rm -f "$data_dir/$target-wal" "$data_dir/$target-shm";; esac
  fi
done
