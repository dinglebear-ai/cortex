#!/usr/bin/env python3
"""Regenerate tracked documentation from its owners, or check without writing.

Schema snapshots mirror contracts/*.schema.json. The existing package and live
inventory generators remain the owners of their respective outputs.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def sync_schemas(root: Path, check: bool) -> bool:
    sources = sorted((root / "contracts").glob("*.schema.json"))
    if not sources:
        raise ValueError("no canonical contracts/*.schema.json found")
    destination = root / "docs/contracts/generated"
    names = {source.name for source in sources}
    orphans = sorted(path.name for path in destination.glob("*.schema.json") if path.name not in names)
    if orphans:
        raise ValueError("schema snapshots without canonical sources: " + ", ".join(orphans))
    # Validate every input before writing any snapshot. Preserve source bytes.
    snapshots = [(source, source.read_bytes()) for source in sources]
    for source, data in snapshots:
        if not isinstance(json.loads(data), dict):
            raise ValueError(f"{source} must contain a JSON schema object")
    current = True
    for source, data in snapshots:
        target = destination / source.name
        if target.is_symlink():
            raise ValueError(f"generated schema must be a regular file: {target}")
        if target.exists() and target.read_bytes() == data:
            continue
        if check:
            print(f"stale or missing: {target.relative_to(root)}", file=sys.stderr)
            current = False
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            print(f"updated: {target.relative_to(root)}")
    return current


def generate(root: Path, check: bool) -> int:
    valid = sync_schemas(root, check)
    flag = ["--check"] if check else []
    commands = [
        ["node", str(root / "packages/cortex-rmcp/scripts/sync-readme.js"), *flag],
        [sys.executable, str(root / "tests/live/generate-docs.py"), *flag],
        [sys.executable, str(root / "scripts/check-integration-contracts.py")],
    ]
    for command in commands:
        result = subprocess.run(command, cwd=root, check=False)
        valid = result.returncode == 0 and valid
    if not valid:
        print("Fix canonical inputs, run `just docs-generate`, then `just docs-check`.", file=sys.stderr)
    return 0 if valid else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="report drift without writing tracked files")
    args = parser.parse_args()
    try:
        return generate(ROOT, args.check)
    except (OSError, ValueError) as error:
        print(f"documentation generation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
