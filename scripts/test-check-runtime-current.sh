#!/usr/bin/env bash
# Lightweight checks for check-runtime-current.sh argument handling. Runtime
# Docker behavior is verified live by the command itself.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKER="${SCRIPT_DIR}/check-runtime-current.sh"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

out="$("${CHECKER}" --help)"
[[ "${out}" == *"--mode auto|docker"* ]] || fail "help omits mode usage"
[[ "${out}" == *"local cache"* ]] || fail "help omits Docker cache semantics"
[[ "${out}" == *"--allow-legacy"* ]] || fail "help omits legacy escape hatch"
[[ "${out}" == *"--allow-local-image"* ]] || fail "help omits local image escape hatch"

set +e
out="$("${CHECKER}" --bogus 2>&1)"
status=$?
set -e
[[ "${status}" -eq 2 ]] || fail "unknown argument exit=${status}, want 2"
[[ "${out}" == *"unknown argument: --bogus"* ]] || fail "unknown argument message missing"

set +e
out="$("${CHECKER}" --mode nope 2>&1)"
status=$?
set -e
[[ "${status}" -eq 2 ]] || fail "invalid mode exit=${status}, want 2"
[[ "${out}" == *"invalid mode: nope"* ]] || fail "invalid mode message missing"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT
cat >"${tmpdir}/docker" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
image="${CORTEX_TEST_IMAGE:-cortex:local-debug}"
case "$*" in
  "ps --filter name=^/cortex$ --format {{.ID}}"*) echo cid ;;
  "inspect cid --format {{ index .Config.Labels \"com.docker.compose.project.working_dir\" }}"*) echo "${CORTEX_TEST_COMPOSE_DIR:-/tmp/legacy-compose}" ;;
  "inspect cid --format {{.Config.Image}}"*) echo "$image" ;;
  "inspect cid --format {{.Image}}"*) echo sha256:debug ;;
  "image inspect "*" --format {{.Id}}"*) echo sha256:debug ;;
  "image inspect "*" --format {{join .RepoDigests \", \"}}"*) echo "" ;;
  "exec cid cortex --version"*) awk -F'"' '/^version = / {print "cortex " $2; exit}' "${PWD}/Cargo.toml" ;;
  *) echo "unexpected docker args: $*" >&2; exit 9 ;;
esac
SH
chmod +x "${tmpdir}/docker"

# A non-canonical Compose dir with a *non-local* ghcr image must FAIL: the
# canonical-dir guard only applies to supported registry images (local/dev
# images are exempt, exercised below).
set +e
out="$(CORTEX_TEST_IMAGE=ghcr.io/dinglebear-ai/cortex:0.1.0 PATH="${tmpdir}:${PATH}" "${CHECKER}" --mode docker 2>&1)"
status=$?
set -e
[[ "${status}" -eq 1 ]] || fail "legacy compose dir exit=${status}, want 1; output=${out}"
[[ "${out}" == *"non-canonical Compose dir"* ]] || fail "legacy compose dir message missing; output=${out}"

set +e
out="$(PATH="${tmpdir}:${PATH}" "${CHECKER}" --mode docker --allow-legacy 2>&1)"
status=$?
set -e
[[ "${status}" -eq 0 ]] || fail "local-debug image exit=${status}, want 0; output=${out}"
[[ "${out}" == *"CURRENT:"* ]] || fail "local-debug image current message missing"

set +e
out="$(CORTEX_TEST_IMAGE=custom/syslog:dev PATH="${tmpdir}:${PATH}" "${CHECKER}" --mode docker --allow-legacy 2>&1)"
status=$?
set -e
[[ "${status}" -eq 1 ]] || fail "custom local image exit=${status}, want 1"
[[ "${out}" == *"unsupported image"* ]] || fail "custom local image message missing"

# A Compose project owned by this checkout may use Compose's generated local
# image name. It remains safe because the checker still compares exact image
# IDs and the embedded binary version.
set +e
out="$(CORTEX_TEST_IMAGE=custom/syslog:dev CORTEX_TEST_COMPOSE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)" PATH="${tmpdir}:${PATH}" "${CHECKER}" --mode docker 2>&1)"
status=$?
set -e
[[ "${status}" -eq 0 ]] || fail "checkout-built image exit=${status}, want 0; output=${out}"
[[ "${out}" == *"CURRENT:"* ]] || fail "checkout-built image current message missing"

# ── Env-file resolution (cortex Issue 2 regression) ──────────────────────
# The deployed compose stack stores its env file one level above the compose
# dir (~/.cortex/.env) and is launched with `--env-file <home>/.env`. The
# runtime-current check must resolve and pass that same env file to
# `docker compose config --images`, otherwise ${CORTEX_VERSION:-...} falls
# back to the compose default and the wrong image tag is compared.

# Build a fake deploy layout: <home>/compose holds the compose file, <home>/.env
# sets CORTEX_VERSION. The mock docker echoes the resolved image tag using
# the CORTEX_VERSION it received in its environment (docker compose loads
# --env-file into the process env before substitution).
# Realistic deploy: image TAG is `main` (set via env file), but the binary
# version reported by `cortex --version` is the semver baked into Cargo.toml.
# So compose tag resolution -> main (matches running image), and the version
# check -> repo semver (matches container). Both must pass => CURRENT, exit 0.
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_VERSION="$(awk -F'"' '/^version = / {print $2; exit}' "${REPO_DIR}/Cargo.toml")"

envtmp="$(mktemp -d)"
trap 'rm -rf "${tmpdir}" "${envtmp}"' EXIT
mkdir -p "${envtmp}/compose"
printf 'CORTEX_VERSION=main\n' >"${envtmp}/.env"
# shellcheck disable=SC2016  # the ${CORTEX_VERSION} is literal compose YAML
printf 'services:\n  cortex:\n    image: ghcr.io/dinglebear-ai/cortex:${CORTEX_VERSION:-0.28.2}\n' \
  >"${envtmp}/compose/docker-compose.yml"

cat >"${envtmp}/docker" <<SH
#!/usr/bin/env bash
set -euo pipefail
REPO_VERSION="${REPO_VERSION}"
SH
cat >>"${envtmp}/docker" <<'SH'
# Capture whether --env-file was passed to `compose config`.
case "$*" in
  *"--env-file"*"config --images"*)
    # docker compose would have loaded the --env-file into substitution.
    # Find the env file path argument and source it to emulate substitution.
    prev=""; envf=""
    for a in "$@"; do
      [[ "$prev" == "--env-file" ]] && envf="$a"
      prev="$a"
    done
    ver="0.28.2"
    [[ -n "$envf" && -f "$envf" ]] && ver="$(awk -F= '/^CORTEX_VERSION=/{print $2; exit}' "$envf")"
    echo "ghcr.io/dinglebear-ai/cortex:${ver}"
    ;;
  *"config --images"*)
    # No --env-file: compose default tag.
    echo "ghcr.io/dinglebear-ai/cortex:0.28.2"
    ;;
  *"ps -q cortex"*) echo cid ;;
  "inspect cid --format {{ index .Config.Labels \"com.docker.compose.project.working_dir\" }}"*)
    echo "${CORTEX_FAKE_COMPOSE_DIR}" ;;
  "inspect cid --format {{.Config.Image}}"*) echo "ghcr.io/dinglebear-ai/cortex:main" ;;
  "inspect cid --format {{.Image}}"*) echo sha256:running ;;
  "image inspect ghcr.io/dinglebear-ai/cortex:main --format {{.Id}}"*) echo sha256:running ;;
  "image inspect ghcr.io/dinglebear-ai/cortex:main --format {{join .RepoDigests \", \"}}"*) echo "" ;;
  "exec cid cortex --version"*) echo "cortex ${REPO_VERSION}" ;;
  *) echo "unexpected docker args: $*" >&2; exit 9 ;;
esac
SH
chmod +x "${envtmp}/docker"

# Direct functional check: run config --images both with and without env-file
# resolution to prove the mock distinguishes the two (sanity for the e2e below).
got_default="$(cd "${envtmp}/compose" && PATH="${envtmp}:${PATH}" docker compose config --images)"
[[ "${got_default}" == "ghcr.io/dinglebear-ai/cortex:0.28.2" ]] \
  || fail "mock default tag wrong: ${got_default}"
got_envfile="$(cd "${envtmp}/compose" && PATH="${envtmp}:${PATH}" docker compose --env-file "${envtmp}/.env" config --images)"
[[ "${got_envfile}" == "ghcr.io/dinglebear-ai/cortex:main" ]] \
  || fail "mock env-file tag wrong: ${got_envfile}"

# End-to-end: the checker must pass --env-file and therefore report CURRENT.
set +e
out="$(
  CORTEX_FAKE_COMPOSE_DIR="${envtmp}/compose" \
  CORTEX_COMPOSE_DIR="${envtmp}/compose" \
  CORTEX_HOME="${envtmp}" \
  PATH="${envtmp}:${PATH}" \
  "${CHECKER}" --mode docker --allow-legacy 2>&1
)"
status=$?
set -e
[[ "${out}" == *"env_file"* ]] || fail "env_file status line missing; output=${out}"
[[ "${out}" == *"cortex:main"* ]] || fail "env-file resolution did not yield main tag; output=${out}"
[[ "${out}" != *"local_image_id missing"* ]] || fail "false local_image_id missing; output=${out}"
[[ "${status}" -eq 0 ]] || fail "env-file deploy should be CURRENT exit=${status}; output=${out}"
[[ "${out}" == *"CURRENT:"* ]] || fail "env-file deploy current message missing; output=${out}"

echo "check-runtime-current.sh env-file resolution tests passed"

echo "check-runtime-current.sh argument tests passed"
