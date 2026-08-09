#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"
TMP="$(mktemp -d)"
cleanup() { rm -rf "${TMP}"; }
trap cleanup EXIT INT TERM

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
make_binary() {
  local path="$1" target="$2" version="$3"
  python3 - "${path}" "${target}" "${version}" <<'PY'
import struct, sys
path, target, version = sys.argv[1:]
if target == "linux":
    data = bytearray(128)
    data[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", data, 18, 62)
else:
    data = bytearray(256)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 128)
    data[128:132] = b"PE\0\0"
    struct.pack_into("<H", data, 132, 0x8664)
data.extend(f"cortex {version}".encode())
open(path, "wb").write(data)
PY
  chmod +x "${path}"
}

mkdir -p "${TMP}/bin" "${TMP}/target/release" "${TMP}/target/x86_64-pc-windows-gnu/release"
cat > "${TMP}/bin/uname" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "${TEST_UNAME:-Linux}"
SH
chmod +x "${TMP}/bin/uname"
make_binary "${TMP}/target/release/cortex" linux "${VERSION}"
make_binary "${TMP}/target/x86_64-pc-windows-gnu/release/cortex.exe" windows "${VERSION}"

run_build() { PATH="${TMP}/bin:${PATH}" CARGO_TARGET_DIR="${TMP}/target" TEST_UNAME="${1}" bash scripts/build-mcpb.sh --target "${2}" --no-build; }
expect_fail() { if "$@" >"${TMP}/failure.out" 2>&1; then echo "expected failure: $*" >&2; exit 1; fi; }

expect_fail run_build Darwin linux
expect_fail run_build MINGW64_NT linux
run_build Linux linux
run_build Linux windows

# Native Windows resolves the host release directory and never demands MinGW.
make_binary "${TMP}/target/release/cortex.exe" windows "${VERSION}"
run_build MINGW64_NT windows

# A renamed ELF and a stale PE must fail before they can replace prior output.
cp "${TMP}/target/release/cortex" "${TMP}/target/x86_64-pc-windows-gnu/release/cortex.exe"
expect_fail run_build Linux windows
make_binary "${TMP}/target/x86_64-pc-windows-gnu/release/cortex.exe" windows "0.0.0"
expect_fail run_build Linux windows
make_binary "${TMP}/target/x86_64-pc-windows-gnu/release/cortex.exe" windows "${VERSION}"

# Isolated mktemp staging must remain correct under concurrent target builds.
run_build Linux linux & linux_pid=$!
run_build Linux windows & windows_pid=$!
wait "${linux_pid}"
wait "${windows_pid}"
python3 - "dist/cortex-${VERSION}-linux.mcpb" "dist/cortex-${VERSION}-windows.mcpb" <<'PY'
import json, sys, zipfile
for path, platform, entry, magic in [
    (sys.argv[1], "linux", "server/cortex", b"\x7fELF"),
    (sys.argv[2], "win32", "server/cortex.exe", b"MZ"),
]:
    with zipfile.ZipFile(path) as archive:
        manifest = json.loads(archive.read("manifest.json"))
        assert manifest["compatibility"]["platforms"] == [platform]
        assert manifest["server"]["entry_point"] == entry
        assert archive.read(entry).startswith(magic)
PY

echo "build-mcpb tests passed"
