#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

NO_BUILD=0
TARGET="host"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-build) NO_BUILD=1 ;;
    --target=*) TARGET="${1#--target=}" ;;
    --target)
      shift
      [ "$#" -gt 0 ] || { echo "--target requires an argument (linux or windows)" >&2; exit 2; }
      TARGET="$1"
      ;;
    --help|-h)
      echo "Usage: scripts/build-mcpb.sh [--target linux|windows] [--no-build]"
      exit 0
      ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

HOST_OS="$(uname -s)"
case "${HOST_OS}" in
  Linux) HOST_PLATFORM="linux" ;;
  MINGW*|MSYS*|CYGWIN*) HOST_PLATFORM="windows" ;;
  *) HOST_PLATFORM="unsupported" ;;
esac

if [ "${TARGET}" = host ]; then
  [ "${HOST_PLATFORM}" != unsupported ] || {
    echo "unsupported host platform for MCPB target auto-detection: ${HOST_OS}" >&2
    exit 1
  }
  TARGET="${HOST_PLATFORM}"
fi
case "${TARGET}" in
  linux|windows) ;;
  *) echo "unsupported MCPB target: ${TARGET} (expected linux or windows)" >&2; exit 2 ;;
esac

# Supported build matrix: Linux native, Linux -> Windows GNU, and Windows native.
case "${HOST_PLATFORM}:${TARGET}" in
  linux:linux)
    RUST_TARGET=""
    PLATFORM="linux"
    BIN_NAME="cortex"
    ;;
  linux:windows)
    RUST_TARGET="x86_64-pc-windows-gnu"
    PLATFORM="win32"
    BIN_NAME="cortex.exe"
    ;;
  windows:windows)
    RUST_TARGET=""
    PLATFORM="win32"
    BIN_NAME="cortex.exe"
    ;;
  *)
    echo "unsupported MCPB host-target combination: ${HOST_OS} -> ${TARGET}" >&2
    exit 1
    ;;
esac

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
MANIFEST_VERSION="$(python3 -c 'import json; print(json.load(open("mcpb/manifest.json"))["version"])')"
if [ "${VERSION}" != "${MANIFEST_VERSION}" ]; then
  echo "mcpb manifest version ${MANIFEST_VERSION} does not match Cargo.toml ${VERSION}" >&2
  exit 1
fi

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
[ -f "${BINARY_PATH}" ] || { echo "missing release binary: ${BINARY_PATH}" >&2; exit 1; }

# Do not trust a release filename: verify format, x86-64 architecture, and the
# product/version marker compiled by src/main.rs before --no-build packaging.
python3 - "${BINARY_PATH}" "${TARGET}" "${VERSION}" <<'PY'
import struct
import sys

path, target, version = sys.argv[1:]
data = open(path, "rb").read()
error = None
if target == "linux":
    if len(data) < 20 or data[:4] != b"\x7fELF":
        error = "expected an ELF executable"
    elif data[4] != 2 or data[5] != 1 or struct.unpack_from("<H", data, 18)[0] != 62:
        error = "expected an x86-64 little-endian ELF executable"
else:
    if len(data) < 64 or data[:2] != b"MZ":
        error = "expected a PE executable"
    else:
        pe = struct.unpack_from("<I", data, 60)[0]
        if pe + 6 > len(data) or data[pe:pe + 4] != b"PE\0\0":
            error = "expected a valid PE executable"
        elif struct.unpack_from("<H", data, pe + 4)[0] != 0x8664:
            error = "expected an x86-64 PE executable"
marker = f"cortex {version}".encode()
if error is None and marker not in data:
    error = f"missing current Cortex version marker {marker.decode()!r}"
if error:
    raise SystemExit(f"invalid release binary {path}: {error}")
PY

mkdir -p dist/mcpb
WORK_DIR="$(mktemp -d "dist/mcpb/cortex-${TARGET}.XXXXXX")"
STAGE_DIR="${WORK_DIR}/stage"
cleanup() { rm -rf "${WORK_DIR}"; }
trap cleanup EXIT INT TERM
OUT_FILE="dist/cortex-${VERSION}-${TARGET}.mcpb"
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
    manifest["user_config"]["data_dir"]["description"] = (
        "Directory containing cortex.db plus WAL/SHM sidecars. The bundled "
        "Windows stdio server reads this database as "
        "CORTEX_DB_PATH=<data_dir>/cortex.db."
    )
json.dump(manifest, sys.stdout, indent=2)
sys.stdout.write("\n")
PY
install -m 755 "${BINARY_PATH}" "${STAGE_DIR}/server/${BIN_NAME}"

MCPB_TOOL_DIR="${WORK_DIR}/tools"
mkdir -p "${MCPB_TOOL_DIR}"
cp tools/mcpb/package.json tools/mcpb/package-lock.json "${MCPB_TOOL_DIR}/"
npm ci --prefix "${MCPB_TOOL_DIR}" --ignore-scripts --no-audit --no-fund >/dev/null
MCPB_CLI="${MCPB_TOOL_DIR}/node_modules/.bin/mcpb"
"${MCPB_CLI}" validate "${STAGE_DIR}/manifest.json"
rm -f "${OUT_FILE}"
"${MCPB_CLI}" pack "${STAGE_DIR}" "${OUT_FILE}"
"${MCPB_CLI}" info "${OUT_FILE}" >/dev/null

echo "Built ${OUT_FILE}"
