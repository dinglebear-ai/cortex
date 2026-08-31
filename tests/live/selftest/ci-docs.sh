#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
python3 "$root/tests/live/generate-docs.py" --check
python3 - "$root" <<'PY'
import json, pathlib, re, sys
root = pathlib.Path(sys.argv[1])
profiles = json.loads((root / "tests/live/contracts/profiles.json").read_text())["profiles"]
just = (root / "Justfile").read_text()
workflow = (root / ".github/workflows/live-qualification.yml").read_text()
if 'live profile="smoke"' not in just:
    raise SystemExit("generic stable Just profile command is missing")
for required in ("if: always()", "sanit", "upload-artifact", "runner.sh --janitor"):
    if required not in workflow:
        raise SystemExit(f"live workflow lacks required cleanup/artifact shape: {required}")
ci = (root / ".github/workflows/ci.yml").read_text()
block = ci[ci.index("mcp-integration:"):ci.index("ci-gate:")]
if "available=false" in block or "skipped::No reachable Docker" in block:
    raise SystemExit("required PR live job still silently skips missing Docker")
print("ci/docs live-suite contract: PASS")
PY
