#!/usr/bin/env bash
# Select remote caching only when every value needed to construct a valid S3
# profile is nonempty. Callers may provide only some secrets/variables (forks,
# Dependabot, or a partially configured organization); those cases must remain
# local-only rather than writing a malformed remote configuration.
set -euo pipefail

required=(
  KACHE_S3_ACCESS_KEY
  KACHE_S3_SECRET_KEY
  KACHE_S3_ENDPOINT
  KACHE_S3_BUCKET
  KACHE_S3_PREFIX
)

for name in "${required[@]}"; do
  value="${!name:-}"
  # These values are written to AWS credentials and TOML basic strings. Reject
  # whitespace/control bytes and the two TOML string delimiters rather than
  # attempting lossy escaping of credentials or operator-supplied identifiers.
  case "$value" in
    ""|*[[:space:]]*|*[[:cntrl:]]*|*\"*|*\\*)
      printf 'local\n'
      exit 0
      ;;
  esac
done

printf 'remote\n'
