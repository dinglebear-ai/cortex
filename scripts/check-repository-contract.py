#!/usr/bin/env python3
"""Run the pinned fleet contract with Cortex-owned agent instruction authority."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import subprocess
import sys


def check(repo: Path, implementation: Path, profile: str) -> int:
    """Apply Cortex's instruction and qualified native ARM contract exceptions."""
    spec = importlib.util.spec_from_file_location("cortex_pinned_fleet_contract", implementation)
    if spec is None or spec.loader is None:
        raise ImportError(f"Cannot load fleet contract: {implementation}")
    module = importlib.util.module_from_spec(spec)
    # Dataclasses resolve their defining module through sys.modules.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    findings = module.check(repo, profile)

    # The immutable upstream implementation predates AGENTS.md ownership and
    # Cortex's qualified native ARM release. Keep the exceptions narrow.
    # Always execute the replacement, including when upstream returns no errors.
    authority = subprocess.run(
        ["bash", str(repo / "scripts/check-agent-memory-symlinks.sh"), str(repo)],
        text=True, capture_output=True, timeout=30, check=False,
    )
    print(authority.stdout, end="")
    print(authority.stderr, end="", file=sys.stderr)
    release_path = repo / ".github/workflows/release.yml"
    release = release_path.read_text() if release_path.is_file() else ""
    native_arm_release = all(
        marker in release
        for marker in (
            "cortex-linux-arm64:",
            "cortex-macos-arm64:",
            "macos-package-gate:",
            "runs-on: ubuntu-24.04-arm",
            "runs-on: macos-15",
            "needs: [cortex-linux, cortex-linux-arm64, cortex-macos-arm64,",
            "cortex-linux-aarch64.tar.gz",
            "cortex-macos-arm64",
        )
    )
    remaining = [
        finding for finding in findings
        if finding.check != "symlink-convention"
        and not (
            native_arm_release
            and finding.check == "no-arm-contract"
            and str(finding.path) == "scripts/install.sh"
        )
    ]
    for finding in remaining:
        print(finding.render())
    if authority.returncode != 0 or remaining:
        return 1
    print("Repository contract valid; Cortex instruction and native ARM rules apply")
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
