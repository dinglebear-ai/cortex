#!/usr/bin/env python3
"""Hermetic checks that the contract adapter replaces only instruction ownership."""

from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
ADAPTER = ROOT / "scripts/check-repository-contract.py"
UPSTREAM = """
class Finding:
    def __init__(self, check, path, message):
        self.check, self.path, self.message = check, path, message
    def render(self):
        return f"{self.check}: {self.path}: {self.message}"

def check(repo, profile):
    findings = [Finding("symlink-convention", "AGENTS.md", "legacy CLAUDE.md ownership")]
    for name in ("docs-frontmatter", "future-contract-check"):
        if (repo / name).exists():
            findings.append(Finding(name, "fixture", "must remain enforced"))
    return findings
"""


class ContractAdapterTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copyfile(ROOT / "scripts/check-agent-memory-symlinks.sh",
                        self.root / "scripts/check-agent-memory-symlinks.sh")
        (self.root / "AGENTS.md").write_text("# Canonical instructions\n")
        for name in ("CLAUDE.md", "GEMINI.md"):
            (self.root / name).symlink_to("AGENTS.md")
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)
        self.implementation = Path(self.temp.name) / "fleet_contract.py"
        self.implementation.write_text(UPSTREAM)

    def invoke(self):
        return subprocess.run(
            [sys.executable, str(ADAPTER), "--repo", str(self.root),
             "--implementation", str(self.implementation), "--profile", "rust"],
            text=True, capture_output=True, timeout=15, check=False,
        )

    def test_replaces_only_legacy_ownership(self):
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("AGENTS.md authority", result.stdout)

    def test_rejects_missing_shared_alias(self):
        (self.root / "GEMINI.md").unlink()
        self.assertNotEqual(self.invoke().returncode, 0)

    def test_enforces_authority_even_without_upstream_findings(self):
        self.implementation.write_text("def check(repo, profile): return []\n")
        (self.root / "CLAUDE.md").unlink()
        (self.root / "CLAUDE.md").write_text("Independent copy\n")
        self.assertNotEqual(self.invoke().returncode, 0)

    def test_preserves_frontmatter_failure(self):
        (self.root / "docs-frontmatter").touch()
        result = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("docs-frontmatter", result.stdout)

    def test_preserves_unknown_future_failures(self):
        (self.root / "future-contract-check").touch()
        result = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("future-contract-check", result.stdout)

    def test_missing_implementation_fails_closed(self):
        self.implementation.unlink()
        self.assertNotEqual(self.invoke().returncode, 0)

    def test_upstream_exception_fails_closed(self):
        self.implementation.write_text("def check(repo, profile): raise RuntimeError('failed')\n")
        self.assertNotEqual(self.invoke().returncode, 0)

    def test_missing_replacement_validator_fails_closed(self):
        (self.root / "scripts/check-agent-memory-symlinks.sh").unlink()
        self.assertNotEqual(self.invoke().returncode, 0)


if __name__ == "__main__":
    unittest.main()
