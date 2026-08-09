#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${CORTEX_OTLP_ENDPOINT:?source deploy/hosts.env first}"

case "$CORTEX_OTLP_ENDPOINT" in
  http://*|https://*) ;;
  *) echo 'CORTEX_OTLP_ENDPOINT must be an http(s) URL' >&2; exit 2 ;;
esac
if [[ "$CORTEX_OTLP_ENDPOINT" == *['&|']* ]]; then
  echo 'CORTEX_OTLP_ENDPOINT contains unsupported template characters' >&2
  exit 2
fi

output_dir="${1:-$repo_root/deploy/rendered}"
mkdir -p "$output_dir"
for source in codex-config.example.toml claude-code-settings.example.json; do
  sed "s|\${CORTEX_OTLP_ENDPOINT}|$CORTEX_OTLP_ENDPOINT|g" \
    "$repo_root/deploy/otel/$source" > "$output_dir/$source"
done
echo "Rendered deployment templates in $output_dir"
