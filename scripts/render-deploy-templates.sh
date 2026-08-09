#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${CORTEX_OTLP_ENDPOINT:?source deploy/hosts.env first}"

if [[ "$CORTEX_OTLP_ENDPOINT" == *example.invalid* ]]; then
  echo 'CORTEX_OTLP_ENDPOINT still contains the non-routable example sentinel' >&2
  exit 2
fi

case "$CORTEX_OTLP_ENDPOINT" in
  http://*|https://*) ;;
  *) echo 'CORTEX_OTLP_ENDPOINT must be an http(s) URL' >&2; exit 2 ;;
esac
if [[ ! "$CORTEX_OTLP_ENDPOINT" =~ ^https?://[A-Za-z0-9][A-Za-z0-9._:-]*(/[A-Za-z0-9._~:/?#@!$()+,\;=%-]*)?$ ]]; then
  echo 'CORTEX_OTLP_ENDPOINT contains unsafe or unsupported URL characters' >&2
  exit 2
fi

output_dir="${1:-$repo_root/deploy/rendered}"
mkdir -p "$output_dir"
staging_dir="$(mktemp -d)"
trap 'rm -rf "$staging_dir"' EXIT
for source in codex-config.example.toml claude-code-settings.example.json; do
  sed "s|\${CORTEX_OTLP_ENDPOINT}|$CORTEX_OTLP_ENDPOINT|g" \
    "$repo_root/deploy/otel/$source" > "$staging_dir/$source"
done
jq empty "$staging_dir/claude-code-settings.example.json"
taplo check "$staging_dir/codex-config.example.toml"
install -m 0644 "$staging_dir/claude-code-settings.example.json" "$output_dir/"
install -m 0644 "$staging_dir/codex-config.example.toml" "$output_dir/"
echo "Rendered deployment templates in $output_dir"
