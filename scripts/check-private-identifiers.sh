#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ "$#" -gt 0 ]; then
  # Explicit paths support hermetic negative fixtures.
  files=("$@")
  fixture_mode=true
else
  mapfile -t files < <(
    git ls-files -z \
      | while IFS= read -r -d '' file; do
          case "$file" in
            AGENTS.md|GEMINI.md|.beads|.beads/*|scripts/check-private-identifiers.sh)
              continue
              ;;
          esac
          printf '%s\n' "$file"
        done
  )
  fixture_mode=false
fi

patterns=(
  dookie squirts shart steamy vivobook
  manatee-triceratops
  100.88.16.79 100.120.242.29 10.1.0.2
  example.internal tv.nashost/cortex nashost.tv
  9C:05:D6:CA:81:3B
)

status=0
for pattern in "${patterns[@]}"; do
  if rg -n --text --fixed-strings --ignore-case -- "$pattern" "${files[@]}"; then
    echo "[private-identifiers] FAIL - private or invalid synthetic identifier found: $pattern" >&2
    status=1
  elif [ "$?" -gt 1 ]; then
    echo "[private-identifiers] FAIL - scan failed for: $pattern" >&2
    status=1
  fi
done

# `aurora.tootie.tv` is the intentionally public design-system registry and is
# retained as external provenance. Other uses of the private host token remain
# forbidden.
set +e
tootie_matches="$(
  rg -n --text --ignore-case --word-regexp -- 'tootie' "${files[@]}" \
    | sed -E 's/aurora\.tootie\.tv/aurora.PUBLIC-REGISTRY.tv/gI' \
    | rg --ignore-case --word-regexp -- 'tootie'
)"
tootie_status=$?
set -e
if [ "$tootie_status" -eq 0 ]; then
  printf '%s\n' "$tootie_matches"
  echo "[private-identifiers] FAIL - private host identifier found: tootie" >&2
  status=1
elif [ "$tootie_status" -gt 1 ]; then
  echo "[private-identifiers] FAIL - tootie allowlist scan failed" >&2
  status=1
fi

# Documentation ranges are valid in examples and redacted history, but never
# in files that directly control CI, image metadata, or Compose routing.
if [ "$fixture_mode" = true ]; then
  active_files=("${files[@]}")
else
  active_files=(config.toml docker-compose.yml config/Dockerfile)
  while IFS= read -r file; do
    active_files+=("$file")
  done < <(git ls-files '.github/*')
fi
if rg -n --text \
  -e '^[[:space:]]*[^#[:space:]].*192\.0\.2\.[0-9]+' \
  -e '^[[:space:]]*[^#[:space:]].*198\.51\.100\.[0-9]+' \
  -e '^[[:space:]]*[^#[:space:]].*203\.0\.113\.[0-9]+' \
  "${active_files[@]}"; then
  echo "[private-identifiers] FAIL - non-routable documentation address found in active configuration" >&2
  status=1
fi

if rg -n --text --ignore-case --word-regexp -- 'nashost' docs/mcp/DEPLOY.md deploy/README.md; then
  echo '[private-identifiers] FAIL - literal synthetic host found in executable deployment guidance' >&2
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "[private-identifiers] OK - public surfaces are scrubbed and active configuration is routable"
fi
exit "$status"
