#!/usr/bin/env python3
"""Generate the checked-in live-suite inventory from authoritative contracts."""
import argparse
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BEGIN = "<!-- BEGIN GENERATED LIVE INVENTORY -->"
END = "<!-- END GENERATED LIVE INVENTORY -->"
KINDS = ("mcp", "rest", "cli", "ingest", "artifact", "browser")


def load_contract(root: Path) -> dict:
    result = subprocess.run([
        "cargo", "run", "--quiet", "--locked", "--manifest-path",
        str(root / "tests/live/surface-exporter/Cargo.toml")
    ], cwd=root, check=True, stdout=subprocess.PIPE, text=True)
    return json.loads(result.stdout)


def render_inventory(contract: dict, profiles: dict) -> str:
    counts = Counter(entry["kind"] for entry in contract["entries"])
    unknown = set(counts) - set(KINDS)
    if unknown:
        raise ValueError("undocumented surface kinds: " + ", ".join(sorted(unknown)))
    lines = [BEGIN, "", "This table is generated from the compiled `SurfaceContract` and `profiles.json`; do not edit counts by hand.", "", "| Inventory | Count |", "|---|---:|"]
    for kind in KINDS:
        lines.append(f"| {kind} surfaces | {counts[kind]} |")
    lines.extend([f"| all surfaces | {len(contract['entries'])} |", f"| runnable profiles | {len(profiles)} |", "", "Profiles: " + ", ".join(f"`{name}`" for name in sorted(profiles)), "", END])
    return "\n".join(lines)


def replace_inventory(text: str, generated: str) -> str:
    if text.count(BEGIN) != 1 or text.count(END) != 1:
        raise ValueError("expected exactly one BEGIN/END GENERATED LIVE INVENTORY marker pair")
    start, end = text.index(BEGIN), text.index(END)
    if start >= end:
        raise ValueError("live inventory markers are reversed")
    return text[:start] + generated + text[end + len(END):]


def update_file(path: Path, generated: str, check: bool) -> bool:
    # read_text() normalizes newlines and could rewrite hand-maintained prose.
    original = path.read_bytes()
    updated = replace_inventory(original.decode("utf-8"), generated).encode("utf-8")
    if original == updated:
        return True
    if check:
        print(f"{path} live inventory is stale; run `just live-docs`", file=sys.stderr)
        return False
    path.write_bytes(updated)
    return True


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--file", type=Path, default=ROOT / "tests/TEST_COVERAGE.md")
    args = parser.parse_args()
    try:
        # Reject damaged ownership markers before an expensive compilation.
        replace_inventory(args.file.read_bytes().decode("utf-8"), "")
        contract = load_contract(ROOT)
        profiles = json.loads((ROOT / "tests/live/contracts/profiles.json").read_text())["profiles"]
        return 0 if update_file(args.file, render_inventory(contract, profiles), args.check) else 1
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"live documentation generation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
