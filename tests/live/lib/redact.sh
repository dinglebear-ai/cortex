#!/usr/bin/env bash

live_secret_file() { printf '%s/secrets.values\n' "${LIVE_RUN_ROOT:?}"; }

live_register_secret() {
  local value="$1" file
  [[ -n "$value" ]] || return 0
  file="$(live_secret_file)"
  [[ ! -L "$file" ]] || { live_die "refusing symlink secret registry"; return; }
  umask 077
  printf '%s\n' "$value" >>"$file"
  chmod 600 "$file"
}

live_redact_stream() {
  python3 "$(dirname "${BASH_SOURCE[0]}")/redact.py" "$(live_secret_file)" "${LIVE_REDACT_MAX_BYTES:-104857600}"
}

live_secret_scan() {
  local root="$1" file secret hit=0
  file="$(live_secret_file)"
  [[ -f "$file" ]] || return 0
  while IFS= read -r secret; do
    [[ -n "$secret" ]] || continue
    if grep -R -F -l --exclude='secrets.values' -- "$secret" "$root" >/dev/null 2>&1; then
      printf 'secret found in persisted artifacts\n' >&2; hit=1
    fi
  done <"$file"
  return "$hit"
}
