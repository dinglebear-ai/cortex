#!/usr/bin/env python3
"""Run the pinned fleet contract with Cortex-owned agent instruction authority."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import re
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
    installer = repo / "install.sh"
    bootstrap = installer.read_text() if installer.is_file() else ""
    # Qualify only the canonical carrier's existing platform and asset contract.
    # Any new target, filename, owner, or changed selector requires explicit review.
    detector = re.search(r"detect_target\(\) \{(.*?)\n\}", bootstrap, re.S)
    selector = re.search(r"asset_name\(\) \{(.*?)\n\}", bootstrap, re.S)
    targets = re.findall(r"^    ([^\n]+)\) printf '([^']+)' ;;$", detector[1], re.M) if detector else []
    assets = re.findall(r"^    ([^\n]+)\) printf '([^']+)'(?: \"\$1\")? ;;$", selector[1], re.M) if selector else []
    canonical_carrier = (
        'REPO="${CORTEX_INSTALL_REPO:-dinglebear-ai/cortex}"' in bootstrap
        and targets == [
            ("linux:x86_64|linux:amd64", "linux-x86_64"),
            ("linux:aarch64|linux:arm64", "linux-aarch64"),
            ("darwin:arm64|darwin:aarch64", "macos-arm64"),
        ]
        and assets == [("macos-arm64", "cortex-macos-arm64"), ("*", "cortex-%s.tar.gz")]
        and all(asset in release for asset in (
            "cortex-linux-x86_64.tar.gz", "cortex-linux-aarch64.tar.gz", "cortex-macos-arm64",
        ))
    )
    adapter_path = repo / "scripts/install.sh"
    adapter = adapter_path.read_text() if adapter_path.is_file() else ""
    delegates_to_canonical = all(marker in adapter for marker in (
        'exec sh "$script_dir/../install.sh" "$@"',
        '${CORTEX_RMCP_REPO:-dinglebear-ai/cortex}',
        'https://raw.githubusercontent.com/$CORTEX_INSTALL_REPO/main/install.sh',
    ))
    remaining = [
        finding for finding in findings
        if finding.check != "symlink-convention"
        and not (
            native_arm_release
            and canonical_carrier
            and finding.check == "no-arm-contract"
            and (str(finding.path) == "install.sh"
                 or (str(finding.path) == "scripts/install.sh" and delegates_to_canonical))
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
