#!/usr/bin/env python3
"""Run the pinned fleet contract with Cortex-owned agent instruction authority."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import subprocess
import sys


def check(repo: Path, implementation: Path, profile: str) -> int:
    """Replace only the legacy CLAUDE-canonical rule; preserve all other findings."""
    spec = importlib.util.spec_from_file_location("cortex_pinned_fleet_contract", implementation)
    if spec is None or spec.loader is None:
        raise ImportError(f"Cannot load fleet contract: {implementation}")
    module = importlib.util.module_from_spec(spec)
    # Dataclasses resolve their defining module through sys.modules.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    findings = module.check(repo, profile)

    # The immutable upstream implementation predates AGENTS.md ownership.
    # Its symlink-convention rule is replaced, not the rest of the contract.
    # Always execute the replacement, including when upstream returns no errors.
    authority = subprocess.run(
        ["bash", str(repo / "scripts/check-agent-memory-symlinks.sh"), str(repo)],
        text=True, capture_output=True, timeout=30, check=False,
    )
    print(authority.stdout, end="")
    print(authority.stderr, end="", file=sys.stderr)
    remaining = [finding for finding in findings if finding.check != "symlink-convention"]
    for finding in remaining:
        print(finding.render())
    if authority.returncode != 0 or remaining:
        return 1
    print("Repository contract valid; AGENTS.md authority replaces the legacy CLAUDE.md rule")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--implementation", type=Path, required=True)
    parser.add_argument("--profile", choices=("rust", "python", "node", "go", "ops"), default="rust")
    args = parser.parse_args()
    try:
        return check(args.repo.resolve(strict=True), args.implementation.resolve(strict=True), args.profile)
    except Exception as error:
        # Missing, incompatible or crashing validators must never report success.
        print(f"Repository contract could not complete: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
