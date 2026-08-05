#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

NO_BUILD=0
TARGET="host"
while [ "$#" -gt 0 ]; do
  arg="$1"
  case "${arg}" in
    --no-build) NO_BUILD=1 ;;
    --target=*) TARGET="${arg#--target=}" ;;
    --target)
      shift
      if [ "$#" -eq 0 ]; then
        echo "--target requires an argument (linux or windows)" >&2
        exit 2
      fi
      TARGET="$1"
      ;;
    --help|-h)
      echo "Usage: scripts/build-mcpb.sh [--target linux|windows] [--no-build]"
      exit 0
      ;;
    *)
      echo "unknown argument: ${arg}" >&2
      exit 2
      ;;
  esac
  shift
done

case "${TARGET}" in
  host)
    case "$(uname -s)" in
      Linux) TARGET="linux" ;;
      MINGW*|MSYS*|CYGWIN*) TARGET="windows" ;;
      *)
        echo "unsupported host platform for MCPB target auto-detection: $(uname -s)" >&2
        exit 1
        ;;
    esac
    ;;
  linux|windows) ;;
  *)
    echo "unsupported MCPB target: ${TARGET} (expected linux or windows)" >&2
    exit 2
    ;;
esac

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
MANIFEST_VERSION="$(python3 -c 'import json; print(json.load(open("mcpb/manifest.json"))["version"])')"
if [ "${VERSION}" != "${MANIFEST_VERSION}" ]; then
  echo "mcpb manifest version ${MANIFEST_VERSION} does not match Cargo.toml ${VERSION}" >&2
  exit 1
fi

case "${TARGET}" in
  linux)
    RUST_TARGET=""
    PLATFORM="linux"
    BIN_NAME="cortex"
    ;;
  windows)
    RUST_TARGET="x86_64-pc-windows-gnu"
    PLATFORM="win32"
    BIN_NAME="cortex.exe"
    ;;
esac

if [ "${NO_BUILD}" -eq 0 ]; then
  if [ -n "${RUST_TARGET}" ]; then
    if ! rustup target list --installed 2>/dev/null | grep -qx "${RUST_TARGET}"; then
      echo "error: rustup target '${RUST_TARGET}' is not installed." >&2
      echo "  Install it with: rustup target add ${RUST_TARGET}" >&2
      exit 1
    fi
    if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
      echo "error: mingw-w64 linker 'x86_64-w64-mingw32-gcc' not found on PATH." >&2
      echo "  Cross-compiling to ${RUST_TARGET} requires a mingw-w64 toolchain." >&2
      echo "  Install it with your package manager, e.g.:" >&2
      echo "    Debian/Ubuntu: sudo apt install gcc-mingw-w64-x86-64" >&2
      echo "    Fedora:        sudo dnf install mingw64-gcc" >&2
      echo "    Arch:          sudo pacman -S mingw-w64-gcc" >&2
      exit 1
    fi
    cargo build --release --target "${RUST_TARGET}"
  else
    cargo build --release
  fi
fi

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ -n "${RUST_TARGET}" ]; then
  BINARY_PATH="${TARGET_DIR}/${RUST_TARGET}/release/${BIN_NAME}"
  CACHE_BINARY_PATH=".cache/cargo/${RUST_TARGET}/release/${BIN_NAME}"
else
  BINARY_PATH="${TARGET_DIR}/release/${BIN_NAME}"
  CACHE_BINARY_PATH=".cache/cargo/release/${BIN_NAME}"
fi
if [ ! -f "${BINARY_PATH}" ] && [ -f "${CACHE_BINARY_PATH}" ]; then
  BINARY_PATH="${CACHE_BINARY_PATH}"
fi
if [ ! -f "${BINARY_PATH}" ]; then
  echo "missing release binary: ${BINARY_PATH}" >&2
  exit 1
fi

STAGE_DIR="dist/mcpb/cortex"
OUT_FILE="dist/cortex-${VERSION}-${TARGET}.mcpb"
rm -rf "${STAGE_DIR}"
mkdir -p "${STAGE_DIR}/server"

python3 - "${TARGET}" "${PLATFORM}" "${BIN_NAME}" mcpb/manifest.json > "${STAGE_DIR}/manifest.json" <<'PY'
import json
import sys

target, platform, bin_name, manifest_path = sys.argv[1:]
with open(manifest_path, encoding="utf-8") as fh:
    manifest = json.load(fh)
entry_point = f"server/{bin_name}"
manifest["compatibility"]["platforms"] = [platform]
manifest["server"]["entry_point"] = entry_point
manifest["server"]["mcp_config"]["command"] = "${__dirname}/" + entry_point
if target == "windows":
    data_dir = manifest["user_config"]["data_dir"]
    data_dir["description"] = (
        "Directory containing cortex.db plus WAL/SHM sidecars. The bundled "
        "Windows stdio server reads this database as "
        "CORTEX_DB_PATH=<data_dir>/cortex.db."
    )
json.dump(manifest, sys.stdout, indent=2)
sys.stdout.write("\n")
PY
install -m 755 "${BINARY_PATH}" "${STAGE_DIR}/server/${BIN_NAME}"

npx --yes @anthropic-ai/mcpb validate "${STAGE_DIR}/manifest.json"
rm -f "${OUT_FILE}"
npx --yes @anthropic-ai/mcpb pack "${STAGE_DIR}" "${OUT_FILE}"
npx --yes @anthropic-ai/mcpb info "${OUT_FILE}" >/dev/null

echo "Built ${OUT_FILE}"
