#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../../../.." && pwd)"; tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
export LIVE_PROJECT_ROOT="$root"
source "$root/tests/live/lib/common.sh"
source "$root/tests/live/phases/artifacts/run.sh"
reject() { if "$@" >/dev/null 2>&1; then echo "artifact mutant survived: $*" >&2; exit 1; fi; }
mkdir -p "$tmp/good/cortex/skills/test" "$tmp/out"
printf '%s\n' '---' 'name: test' 'description: test' '---' >"$tmp/good/cortex/skills/test/SKILL.md"
(cd "$tmp/good" && tar -czf "$tmp/good.tgz" cortex)
python3 "$root/tests/live/phases/artifacts/safe_extract.py" "$tmp/good.tgz" "$tmp/out" >/dev/null
python3 - "$tmp/bad.tgz" <<'PY'
import io, sys, tarfile
with tarfile.open(sys.argv[1], "w:gz") as archive:
    info=tarfile.TarInfo("../escape"); info.size=1
    archive.addfile(info,io.BytesIO(b"x"))
PY
if python3 "$root/tests/live/phases/artifacts/safe_extract.py" "$tmp/bad.tgz" "$tmp/out2" >/dev/null 2>&1; then echo 'unsafe archive accepted' >&2; exit 1; fi

digest="$(shasum -a 256 "$tmp/good.tgz" | awk '{print $1}')"
jq -cn --arg path "$tmp/good.tgz" --arg origin "file://$tmp/good.tgz" --arg digest "$digest" \
  '{schema:"cortex-live-artifact-manifest-v1",release:"3.15.0",artifacts:[{name:"good.tgz",kind:"plugin",origin:$origin,path:$path,sha256:$digest,platform:"any"}]}' >"$tmp/manifest.json"
artifact_validate_manifest_schema "$tmp/manifest.json"
jq '.artifacts[0].name="../../escape"' "$tmp/manifest.json" >"$tmp/traversal.json"
reject artifact_validate_manifest_schema "$tmp/traversal.json"

trusted_identity='https://github.com/dinglebear-ai/cortex/.github/workflows/release.yml@refs/tags/v3.15.0'
trusted_issuer='https://token.actions.githubusercontent.com'
container_digest='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
container_ref="ghcr.io/dinglebear-ai/cortex@sha256:$container_digest"
container_origin="https://ghcr.io/v2/dinglebear-ai/cortex/manifests/sha256:$container_digest"
row="$(jq -cn --arg identity "$trusted_identity" --arg issuer "$trusted_issuer" '{signature:{type:"cosign-image",identity:$identity,issuer:$issuer,bundle:"bundle.json"}}')"
artifact_assert_trusted_remote_identity "$row" 3.15.0 container "$container_ref" "$container_origin" "$container_digest"
bad_row="$(jq -c '.signature.issuer="https://attacker.invalid"' <<<"$row")"
reject artifact_assert_trusted_remote_identity "$bad_row" 3.15.0 container "$container_ref" "$container_origin" "$container_digest"
bad_row="$(jq -c '.signature.identity="https://github.com/attacker/repo/.github/workflows/release.yml@refs/tags/v3.15.0"' <<<"$row")"
reject artifact_assert_trusted_remote_identity "$bad_row" 3.15.0 container "$container_ref" "$container_origin" "$container_digest"
reject artifact_assert_trusted_remote_identity "$row" 3.15.0 container "ghcr.io/dinglebear-ai/cortex@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" "$container_origin" "$container_digest"
reject artifact_assert_trusted_remote_identity "$row" 3.15.0 container "$container_ref" "https://ghcr.io/v2/dinglebear-ai/cortex/manifests/latest" "$container_digest"

blob_row="$(jq -cn --arg identity "$trusted_identity" --arg issuer "$trusted_issuer" '{signature:{type:"cosign-blob",identity:$identity,issuer:$issuer,bundle:"bundle.json"}}')"
artifact_assert_trusted_remote_identity "$blob_row" 3.15.0 archive "$tmp/good.tgz" "https://github.com/dinglebear-ai/cortex/releases/download/v3.15.0/good.tgz" "$digest"
reject artifact_assert_trusted_remote_identity "$blob_row" 3.15.0 archive "$tmp/good.tgz" "https://github.com/dinglebear-ai/cortex/releases/download/v3.14.0/good.tgz" "$digest"
grep -q 'credentials_inherited:false' "$root/tests/live/phases/artifacts/run.sh"
echo 'artifact selftest: PASS'
