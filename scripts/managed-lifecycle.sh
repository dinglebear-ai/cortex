#!/bin/sh
# Managed Compose recovery. Never source .env as executable shell code.
set -eu
umask 077
action=${1:?action required}
home=${2:?managed home required}
stamp=${3:-}
case "$home" in /*) ;; *) echo 'managed home must be absolute' >&2; exit 64;; esac
case "$home" in /|*/../*|*/..|*'
'*) echo 'unsafe managed home' >&2; exit 64;; esac
compose_file=$home/compose/docker-compose.yml
env_file=$home/.env
compose() {
  if test -f "$home/compose/docker-compose.recovery.yml"; then
    if test -f "$home/compose/docker-compose.override.yml"; then
      docker compose --project-directory "$home/compose" --env-file "$env_file" -f "$compose_file" -f "$home/compose/docker-compose.override.yml" -f "$home/compose/docker-compose.recovery.yml" "$@"
    else
      docker compose --project-directory "$home/compose" --env-file "$env_file" -f "$compose_file" -f "$home/compose/docker-compose.recovery.yml" "$@"
    fi
  elif test -f "$home/compose/docker-compose.override.yml"; then
    docker compose --project-directory "$home/compose" --env-file "$env_file" -f "$compose_file" -f "$home/compose/docker-compose.override.yml" "$@"
  else
    docker compose --project-directory "$home/compose" --env-file "$env_file" -f "$compose_file" "$@"
  fi
}
lock_owned=false
restart_needed=false
running=false
incomplete_bundle=
cleanup_lifecycle() {
  status=$?
  trap - EXIT
  if test "$restart_needed" = true && test "$running" = true; then
    compose start cortex >/dev/null || { echo 'could not restart original service; inspect the owned target before retrying' >&2; status=1; }
  fi
  if test -n "$incomplete_bundle" && ! test -f "$incomplete_bundle/VERIFIED"; then
    rm -rf "$incomplete_bundle"
  fi
  if test "$lock_owned" = true; then
    rm -f "$home/.lifecycle-lock/pid"
    rmdir "$home/.lifecycle-lock" || status=1
  fi
  exit "$status"
}
acquire_lock() {
  if test -n "${CORTEX_LIFECYCLE_TOKEN:-}" && test -f "$home/.lifecycle-lock/token" \
     && test "$(cat "$home/.lifecycle-lock/token")" = "$CORTEX_LIFECYCLE_TOKEN"; then
    lock_owned=false
  elif test -n "${CORTEX_LIFECYCLE_PARENT:-}" && test -f "$home/.lifecycle-lock/pid" \
     && test "$(cat "$home/.lifecycle-lock/pid")" = "$CORTEX_LIFECYCLE_PARENT" \
     && kill -0 "$CORTEX_LIFECYCLE_PARENT" 2>/dev/null; then
    # A restore takes a fresh snapshot under its already-owned parent lock.
    lock_owned=false
  else
    mkdir "$home/.lifecycle-lock" 2>/dev/null || { echo 'another lifecycle operation owns the target; inspect .lifecycle-lock/pid before retrying' >&2; exit 75; }
    printf '%s\n' "$$" > "$home/.lifecycle-lock/pid"
    lock_owned=true
  fi
  trap cleanup_lifecycle EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM HUP
}

# Do not touch a named container merely because its name is familiar.
owned_container() {
  docker info >/dev/null || { echo 'Docker daemon unavailable; ownership cannot be established' >&2; exit 69; }
  if ! docker inspect cortex >/dev/null 2>&1; then return 1; fi
  files=$(docker inspect -f '{{ index .Config.Labels "com.docker.compose.project.config_files" }}' cortex)
  service=$(docker inspect -f '{{ index .Config.Labels "com.docker.compose.service" }}' cortex)
  case ",$files," in *",$compose_file,"*) ;; *) echo 'Cortex container belongs to another Compose target; refusing mutation' >&2; exit 65;; esac
  test "$service" = cortex || { echo 'container is not the Cortex Compose service' >&2; exit 65; }
}
# Resolve the backup mount from the actual container, not the caller's environment.
backup_root() {
  docker inspect -f '{{range .Mounts}}{{if eq .Destination "/backups"}}{{.Source}}{{end}}{{end}}' cortex
}
writable_mounts() {
  docker inspect -f '{{range .Mounts}}{{if .RW}}{{.Destination}}|{{.Type}}|{{.Source}}{{println}}{{end}}{{end}}' cortex
}
verify_layout() {
  mounts=$(writable_mounts)
  test -n "$mounts" || { echo 'no managed writable mounts found' >&2; exit 65; }
  while IFS='|' read -r destination kind source; do
    test -n "$destination" || continue
    case "$destination" in
      /data|/backups|/cortex-home) ;;
      *) echo "custom writable mount $destination is outside verified managed recovery coverage; back it up explicitly before changing the deployment" >&2; exit 65;;
    esac
    test -n "$kind" && test -n "$source" || { echo 'invalid writable mount identity' >&2; exit 65; }
  done <<__CORTEX_MOUNTS__
$mounts
__CORTEX_MOUNTS__
}
# Retain verified recovery points only after a replacement is verified. Operators
# can pin a bundle by placing PROTECTED inside it; pins are never pruned.
prune_bundles() {
  retain=${CORTEX_BACKUP_RETAIN_COUNT:-16}
  case "$retain" in ''|*[!0-9]*) echo 'invalid recovery retention count' >&2; exit 64;; esac
  test "$retain" -ge 1 || { echo 'recovery retention must retain at least one bundle' >&2; exit 64; }
  belongs_to_home() {
    test -f "$1/managed-home" && ! test -L "$1/managed-home" &&
      test "$(cat "$1/managed-home")" = "$home"
  }
  list=$(mktemp "$root/.retention.XXXXXX")
  ls -td "$root"/recovery-* | while IFS= read -r candidate; do
    test -d "$candidate" && ! test -L "$candidate" || continue
    belongs_to_home "$candidate" || continue
    test ! -L "$candidate/VERIFIED" && test ! -L "$candidate/PROTECTED" || continue
    candidate_stamp=${candidate##*/recovery-}
    case "$candidate_stamp" in ''|*[!A-Za-z0-9_.-]*) continue;; esac
    if test -f "$candidate/VERIFIED" && test "$(cat "$candidate/VERIFIED")" = "$candidate_stamp"; then
      printf '%s\n' "$candidate_stamp"
    fi
  done > "$list"
  count=0
  while IFS= read -r candidate_stamp; do
    count=$((count + 1))
    candidate=$root/recovery-$candidate_stamp
    if test "$count" -gt "$retain" && test "$candidate" != "$bundle" &&
       belongs_to_home "$candidate" && test -f "$candidate/VERIFIED" &&
       test ! -L "$candidate/VERIFIED" && test "$(cat "$candidate/VERIFIED")" = "$candidate_stamp" &&
       ! test -e "$candidate/PROTECTED" && ! test -L "$candidate/PROTECTED"; then
      rm -rf "$candidate"
    fi
  done < "$list"
  rm -f "$list"
  # Failed bundles are never recovery candidates; remove old abandoned work,
  # while the target lock excludes a live producer.
  for candidate in "$root"/recovery-*; do
    test -d "$candidate" && ! test -L "$candidate" || continue
    belongs_to_home "$candidate" || continue
    if ! test -e "$candidate/VERIFIED" && ! test -L "$candidate/VERIFIED" &&
       ! test -e "$candidate/PROTECTED" && ! test -L "$candidate/PROTECTED"; then
      if test -n "$(find "$candidate" -maxdepth 0 -mtime +0 -print)"; then rm -rf "$candidate"; fi
    fi
  done
}
space_preflight() {
  retain=${CORTEX_BACKUP_RETAIN_COUNT:-16}
  case "$retain" in ''|*[!0-9]*) echo 'invalid recovery retention count' >&2; exit 64;; esac
  test "$retain" -ge 1 || { echo 'recovery retention must retain at least one bundle' >&2; exit 64; }
  reserve=${CORTEX_BACKUP_MIN_FREE_MB:-1024}
  case "$reserve" in ''|*[!0-9]*) echo 'invalid recovery free-space reserve' >&2; exit 64;; esac
  # Snapshot plus integrity-check staging temporarily consumes two full copies.
  sizes=$(compose run --rm --no-deps --pull never --user 0:0 --entrypoint sh cortex -c 'du -sk --exclude=/cortex-home/backups --exclude=/cortex-home/data --exclude=".before-restore*" --exclude=".restore*" /data /cortex-home | awk "{sum+=\$1} END {print sum}"; df -Pk /backups | awk "NR==2 {print \$4}"')
  used=$(printf '%s\n' "$sizes" | sed -n '1p')
  available=$(printf '%s\n' "$sizes" | sed -n '2p')
  case "$used:$available" in *[!0-9:]*|:*|*:) echo 'cannot establish recovery storage capacity' >&2; exit 65;; esac
  test "$available" -ge "$((used * 2 + reserve * 1024))" || { echo 'insufficient free space for a verified snapshot and reserve; service was not stopped' >&2; exit 65; }
}
validate_stamp() {
  case "$stamp" in ''|*[!A-Za-z0-9_.-]*|.|..) echo 'invalid recovery timestamp' >&2; exit 64;; esac
}
case "$action" in
  preflight)
    if owned_container; then compose config --quiet; echo 'owned managed deployment verified'; else echo 'first installation: no existing container'; fi
    ;;
  prepare|create)
    if ! owned_container; then
      if test "$action" = prepare; then echo 'no existing managed container; recovery snapshot not required'; exit 0; fi
      echo 'no owned managed container to back up' >&2; exit 65
    fi
    compose config --quiet
    verify_layout
    acquire_lock
    root=$(backup_root)
    test -n "$root" && test "$root" != / && test -d "$root" || { echo 'managed backup mount is missing or unsafe' >&2; exit 65; }
    space_preflight
    # Each snapshot is isolated. Never overwrite a previous recovery bundle.
    stamp=$(date -u +%Y-%m-%d-%H%M%S)-$$
    bundle=$root/recovery-$stamp
    mkdir "$bundle"
    incomplete_bundle=$bundle
    chmod 700 "$bundle"
    image=$(docker inspect -f '{{.Image}}' cortex)
    test -n "$image"
    printf '%s\n' "$image" > "$bundle/image"
    printf '%s\n' "$home" > "$bundle/managed-home"
    docker inspect -f '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Type}}|{{.Source}}{{end}}{{end}}' cortex > "$bundle/data-mount"
    printf '%s\n' "$(writable_mounts)" > "$bundle/writable-mounts"
    test -s "$bundle/data-mount" || { echo 'managed container has no /data mount' >&2; exit 65; }
    running=$(docker inspect -f '{{.State.Running}}' cortex)
    restart_original() {
      if test "$running" = true; then compose start cortex >/dev/null || { echo 'backup cleanup could not restart original service; inspect the owned target' >&2; return 1; }; fi
    }
    restart_needed=true
    # Freeze all writers so DB, auth, keys, checkpoints and configuration share
    # one recovery point. Full /data includes custom auth paths inside the volume.
    compose stop -t 60 cortex >/dev/null
    tar -cf "$bundle/config.tar" -C "$home" .env compose
    if test -f "$home/config.toml"; then tar -rf "$bundle/config.tar" -C "$home" config.toml; fi
    compose run --rm --no-deps --pull never --user 0:0 --entrypoint sh cortex -s -- "$stamp" <<'SNAPSHOT'
set -eu
umask 077
stamp=$1
bundle=/backups/recovery-$stamp
test -d "$bundle"
# Preserve additional auth/key stores under the managed home without including
# nested data/backup mounts or the source recovery bundle itself.
home_auth=$(mktemp -d "$bundle/.home-auth.XXXXXX")
find /cortex-home -samefile /data -prune -o -samefile /backups -prune -o -path /cortex-home/data -prune -o -path /cortex-home/backups -prune -o -path '/cortex-home/.restore-*' -prune -o -path '/cortex-home/.before-restore-*' -prune -o -type f -print | while IFS= read -r file; do
  selected=false
  case "$file" in *.db|*.pem|*.key) selected=true;; esac
  header=$(head -c 15 "$file")
  if test "$header" = 'SQLite format 3'; then selected=true; fi
  case "$header" in '-----BEGIN '*) selected=true;; esac
  if test "$selected" = true; then
    relative=${file#/cortex-home/}
    mkdir -p "$home_auth/$(dirname "$relative")"
    cp -p "$file" "$home_auth/$relative"
    # Include SQLite sidecars from the same stopped instant.
    for suffix in -wal -shm; do
      if test -f "$file$suffix"; then cp -p "$file$suffix" "$home_auth/$relative$suffix"; fi
    done
  fi
done
tar -cf "$bundle/home-auth.tar" -C "$home_auth" .
rm -rf "$home_auth"
# Capture stopped data, including SQLite sidecars, then verify the copied DBs.
# Source files are never opened by SQLite during verification.
tar --exclude='./.before-restore.*' --exclude='./.restore.*' -cf "$bundle/data.tar" -C /data .
if tar -tvf "$bundle/data.tar" | grep -Eq '^[lh]'; then echo 'managed data contains links; recovery requires regular files and directories' >&2; exit 65; fi
stage=$(mktemp -d "$bundle/.verify.XXXXXX")
trap 'rm -rf "$stage"' EXIT
tar -xf "$bundle/data.tar" -C "$stage"
find "$stage" -type f -exec sh -c '
  for db do
    case "$db" in *.db) ;; *) test "$(head -c 15 "$db")" = "SQLite format 3" || continue;; esac
    result=$(sqlite3 "$db" "PRAGMA integrity_check;")
    test "$result" = ok || { echo "recovery database integrity check failed" >&2; exit 1; }
  done
' sh {} +
# A missing primary DB is not a verified backup.
primary=${CORTEX_DB_PATH:-/data/cortex.db}
case "$primary" in /data/*) primary=${primary#/data/};; *) echo 'primary DB must be inside the managed /data volume' >&2; exit 65;; esac
case "$primary" in ../*|*/../*|*/..|/*|'') echo 'unsafe primary DB path' >&2; exit 65;; esac
test -s "$stage/$primary" || { echo 'primary database missing from managed data volume' >&2; exit 65; }
test "$(sqlite3 "$stage/$primary" 'PRAGMA integrity_check;')" = ok || { echo 'primary recovery database integrity check failed' >&2; exit 65; }
printf '%s\n' "$primary" > "$bundle/primary-db"
cd "$bundle"
sha256sum data.tar home-auth.tar config.tar image managed-home primary-db data-mount writable-mounts > SHA256SUMS
sha256sum -c SHA256SUMS >/dev/null
printf '%s\n' "$stamp" > VERIFIED
chown -R "$(stat -c '%u:%g' "$bundle")" "$bundle"
SNAPSHOT
    test -s "$bundle/VERIFIED"
    chmod 600 "$bundle"/*
    restart_original
    restart_needed=false
    prune_bundles
    echo "verified recovery snapshot: $bundle (restore with cortex setup backup restore $stamp --yes --home $home)"
    ;;
  restore|rollback)
    validate_stamp
    owned_container || { echo 'no owned container; recover its saved Compose configuration first' >&2; exit 65; }
    root=$(backup_root)
    bundle=$root/recovery-$stamp
    test -f "$bundle/VERIFIED" && test "$(cat "$bundle/VERIFIED")" = "$stamp" || { echo 'snapshot is incomplete or unverified' >&2; exit 65; }
    test "$(cat "$bundle/managed-home")" = "$home" || { echo 'snapshot belongs to another managed home' >&2; exit 65; }
    current_mount=$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Type}}|{{.Source}}{{end}}{{end}}' cortex)
    test -n "$current_mount" && test "$(cat "$bundle/data-mount")" = "$current_mount" || { echo 'snapshot belongs to a different data mount; refusing restore' >&2; exit 65; }
    test "$(cat "$bundle/writable-mounts")" = "$(writable_mounts)" || { echo 'snapshot writable mount identities differ from the current deployment; refusing restore' >&2; exit 65; }
    # Verify archive shape before changing any active data or configuration.
    tar -tf "$bundle/config.tar" | while IFS= read -r name; do
      case "$name" in .env|compose|compose/|compose/*|config.toml) ;; *) echo 'unexpected recovery config archive entry' >&2; exit 65;; esac
      case "$name" in */../*|*/..) exit 65;; esac
    done
    if tar -tvf "$bundle/config.tar" | grep -Eq '^[lh]'; then echo 'configuration archive contains links' >&2; exit 65; fi
    image=$(cat "$bundle/image")
    case "$image" in sha256:[a-f0-9]*) ;; *) echo 'invalid recovery image identity' >&2; exit 65;; esac
    docker image inspect "$image" >/dev/null || { echo 'previous image unavailable; recover it before restoring' >&2; exit 65; }
    # Validate every artifact before stopping the current runtime. The original
    # image is pinned for this one-off verification/restore container.
    check=$home/.recovery-image.yml
    printf 'services:\n  cortex:\n    image: "%s"\n' "$image" > "$check"
    recovery_compose() {
      if test -f "$home/compose/docker-compose.override.yml"; then
        docker compose --project-directory "$home/compose" --env-file "$env_file" -f "$compose_file" -f "$home/compose/docker-compose.override.yml" -f "$check" "$@"
      else
        docker compose --project-directory "$home/compose" --env-file "$env_file" -f "$compose_file" -f "$check" "$@"
      fi
    }
    recovery_compose run --rm --no-deps --pull never --user 0:0 --entrypoint sh cortex -c 'cd "$1"; sha256sum -c SHA256SUMS' sh "/backups/recovery-$stamp" >/dev/null
    # Retain a fresh snapshot of current data under the same lifecycle lock.
    acquire_lock
    touch "$bundle/PROTECTED"
    CORTEX_LIFECYCLE_PARENT=$$ sh "$home/managed-lifecycle.sh" create "$home"
    running=$(docker inspect -f '{{.State.Running}}' cortex)
    restart_needed=true
    compose stop -t 60 cortex >/dev/null
    recovery_compose run --rm --no-deps --pull never --user 0:0 --entrypoint sh cortex -s -- "$stamp" <<'RESTORE'
set -eu
umask 077
bundle=/backups/recovery-$1
cd "$bundle"
sha256sum -c SHA256SUMS >/dev/null
stage=$(mktemp -d /data/.restore.XXXXXX)
trap 'rm -rf "$stage"' EXIT
# Reject archive traversal or links before any extraction.
for archive in data.tar home-auth.tar; do
tar -tf "$archive" | while IFS= read -r name; do
  case "$name" in /*|../*|*/../*|*/..) exit 65;; esac
done
if tar -tvf "$archive" | grep -Eq '^[lh]'; then echo 'recovery archive contains links' >&2; exit 65; fi
done
tar -xf data.tar -C "$stage"
find "$stage" -type f -exec sh -c 'for db do case "$db" in *.db) ;; *) test "$(head -c 15 "$db")" = "SQLite format 3" || continue;; esac; test "$(sqlite3 "$db" "PRAGMA integrity_check;")" = ok || exit 1; done' sh {} +
primary=$(cat "$bundle/primary-db")
case "$primary" in ../*|*/../*|*/..|/*|'') echo 'unsafe primary DB path' >&2; exit 65;; esac
test -s "$stage/$primary"
test "$(sqlite3 "$stage/$primary" 'PRAGMA integrity_check;')" = ok || exit 65
home_stage=$(mktemp -d /cortex-home/.restore-auth.XXXXXX)
tar -xf "$bundle/home-auth.tar" -C "$home_stage"
find "$home_stage" -type f -exec sh -c 'for db do case "$db" in *.db) ;; *) test "$(head -c 15 "$db")" = "SQLite format 3" || continue;; esac; test "$(sqlite3 "$db" "PRAGMA integrity_check;")" = ok || exit 1; done' sh {} +
# Keep the old data on the same volume for recovery from a filesystem error.
previous=$(mktemp -d /data/.before-restore.XXXXXX)
old_complete=false
committed=false
home_previous=$(mktemp -d /cortex-home/.before-restore-auth.XXXXXX)
home_backed_up=false
home_manifest=$home_previous/manifest
# SQLite sidecars form one state with each database, including their absence.
find "$home_stage" -type f -print | while IFS= read -r source; do
  relative=${source#"$home_stage/"}
  printf '%s\n' "$relative"
  if test "$(head -c 15 "$source")" = 'SQLite format 3'; then
    printf '%s\n' "$relative-wal" "$relative-shm"
  fi
done | sort -u > "$home_manifest"
recover_staged_files() {
  status=$?
  trap - EXIT
  if test "$committed" != true; then
    if test "$home_backed_up" = true; then
      while IFS= read -r relative; do
        if test -f "$home_previous/$relative"; then
          cp -p "$home_previous/$relative" "/cortex-home/$relative" || { echo 'home auth recovery failed; prior files retained in .before-restore-auth directory' >&2; exit 1; }
        else
          rm -f "/cortex-home/$relative"
        fi
      done < "$home_manifest" || status=1
    fi
    if test "$old_complete" = true; then
      for entry in /data/* /data/.[!.]* /data/..?*; do
        test -e "$entry" || continue
        case "$entry" in "$stage"|"$previous"|/data/.before-restore.*) continue;; esac
        rm -rf -- "$entry"
      done
    fi
    for entry in "$previous"/* "$previous"/.[!.]* "$previous"/..?*; do
      test -e "$entry" || continue
      mv "$entry" /data/ || { echo 'filesystem recovery failed; original files retained in .before-restore directory' >&2; status=1; }
    done
  fi
  rm -rf "$stage" "$home_stage"
  exit "$status"
}
trap recover_staged_files EXIT
# Stage every previous auth file before any current auth store is replaced.
while IFS= read -r relative; do
  mkdir -p "$home_previous/$(dirname "$relative")"
  if test -f "/cortex-home/$relative"; then cp -p "/cortex-home/$relative" "$home_previous/$relative"; fi
done < "$home_manifest"
home_backed_up=true
for entry in /data/* /data/.[!.]* /data/..?*; do
  test -e "$entry" || continue
  case "$entry" in "$stage"|"$previous"|/data/.before-restore.*) continue;; esac
  mv "$entry" "$previous/"
done
old_complete=true
for entry in "$stage"/* "$stage"/.[!.]* "$stage"/..?*; do
  test -e "$entry" || continue
  mv "$entry" /data/
done
# Host-home auth stores share the same verified recovery point. Retain a
# private copy of existing stores before replacing them; no schema runs yet.
while IFS= read -r relative; do
  mkdir -p "/cortex-home/$(dirname "$relative")"
  if test -f "$home_stage/$relative"; then
    cp -p "$home_stage/$relative" "/cortex-home/$relative"
  else
    rm -f "/cortex-home/$relative"
  fi
done < "$home_manifest"
committed=true
echo 'previous data retained in the data volume; no automatic database rollback'
RESTORE
    # Data has committed. A failure after this point leaves the server stopped
    # for explicit recovery rather than restarting a runtime against new data.
    restart_needed=false
    # Restore saved configuration only after staged database verification passes.
    tar -tf "$bundle/config.tar" | while IFS= read -r name; do
      case "$name" in .env|compose|compose/*|config.toml) ;; *) echo 'unexpected recovery config archive entry' >&2; exit 65;; esac
      case "$name" in */../*|*/..) exit 65;; esac
    done
    if tar -tvf "$bundle/config.tar" | grep -Eq '^[lh]'; then echo 'configuration archive contains links' >&2; exit 65; fi
    config_stage=$(mktemp -d "$home/.restore-config.XXXXXX")
    config_previous=$(mktemp -d "$home/.before-restore-config.XXXXXX")
    tar -xf "$bundle/config.tar" -C "$config_stage"
    config_moved=false
    config_committed=false
    recover_configuration() {
      status=$?
      trap - EXIT
      if test "$config_committed" != true; then
        for name in .env compose config.toml; do
          if test "$config_moved" = true; then rm -rf "$home/$name"; fi
          if test -e "$config_previous/$name"; then mv "$config_previous/$name" "$home/$name" || status=1; fi
        done
      fi
      rm -rf "$config_stage"
      if test "$lock_owned" = true; then rm -f "$home/.lifecycle-lock/pid"; rmdir "$home/.lifecycle-lock" || status=1; fi
      exit "$status"
    }
    trap recover_configuration EXIT
    for name in .env compose config.toml; do
      if test -e "$home/$name"; then mv "$home/$name" "$config_previous/$name"; fi
    done
    config_moved=true
    for name in .env compose config.toml; do
      if test -e "$config_stage/$name"; then mv "$config_stage/$name" "$home/$name"; fi
    done
    config_committed=true
    rm -rf "$config_stage"
    trap cleanup_lifecycle EXIT
    # Always restore with the original image: a schema snapshot and its runtime
    # form one recovery point. Preserve all other operator overrides.
    # Leave the explicit recovery pin in place for subsequent managed commands.
    if test -f "$home/compose/docker-compose.override.yml"; then
      cp "$home/compose/docker-compose.override.yml" "$home/compose/docker-compose.override.yml.before-recovery"
    fi
    cp "$check" "$home/compose/docker-compose.recovery.yml"
    compose up -d --no-build cortex >/dev/null
    echo 'Recovery image is pinned in docker-compose.recovery.yml; remove this pin deliberately before upgrading.'
    # compose() includes the retained recovery pin below.
    echo "restored verified snapshot $stamp; original current data is retained"
    ;;
  schedule-install|schedule-check|schedule-remove)
    test -f "$compose_file" && test -f "$env_file" || { echo 'managed deployment files missing' >&2; exit 65; }
    command -v crontab >/dev/null || { echo 'user crontab is unavailable; install a cron service or schedule cortex backup create using your service manager' >&2; exit 69; }
    # A stable installed binary is supplied by Rust. Never depend on a checkout.
    bin=${CORTEX_BIN:?stable installed Cortex binary required}
    marker=$(printf '%s' "$home" | cksum | awk '{print $1}')
    marker="cortex-backup-$marker"
    schedule=$(mktemp)
    trap 'rm -f "$schedule" "$schedule.new"' EXIT
    # Only an absent crontab is harmless; other failures must be visible.
    if ! crontab -l > "$schedule" 2> "$schedule.new"; then
      if grep -qi 'no crontab' "$schedule.new"; then : > "$schedule"; else cat "$schedule.new" >&2; exit 1; fi
    fi
    if test "$action" = schedule-check; then
      if grep -F "# $marker" "$schedule"; then exit 0; fi
      echo 'backup schedule is not installed' >&2; exit 1
    fi
    awk -v marker="# $marker" 'index($0,marker)==0' "$schedule" > "$schedule.new"
    if test "$action" = schedule-install; then
      # Cron treats % specially even inside quotes, so reject it in paths.
      case "$bin$home" in *%*|*"'"*|*'
'*) echo 'cron paths must not contain percent, quote or newline' >&2; exit 64;; esac
      printf "0 */6 * * * '%s' setup backup create --home '%s' >> '%s/backup-schedule.log' 2>&1 # %s\n" "$bin" "$home" "$home" "$marker" >> "$schedule.new"
    fi
    crontab "$schedule.new"
    echo "$action completed for $home"
    ;;
  *) echo 'unknown lifecycle action' >&2; exit 64;;
esac
