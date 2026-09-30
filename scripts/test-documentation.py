#!/usr/bin/env python3
"""Hermetic regressions for documentation layout, navigation, and local links."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("documentation", ROOT / "scripts/documentation.py")
docs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(docs)


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.files = []
        self.add("README.md", "# Project\n")
        self.add("docs/README.md", "# Documentation\n")

    def add(self, name, content):
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
        if name not in self.files:
            self.files.append(name)
        return target

    def audit(self):
        return docs.audit(self.root, self.files)

    def kinds(self):
        return {issue["kind"] for issue in self.audit()["errors"]}

    def indexes(self, check):
        paths = sorted(str(p.relative_to(self.root)) for p in self.root.rglob("*") if p.is_file())
        with patch.object(docs, "tracked_files", return_value=paths):
            return docs.write_indexes(self.root, check)


class LayoutTests(Fixture):
    def test_valid_lowercase_layout(self):
        self.add("docs/guides/getting-started.md", "# Getting started\n")
        self.assertEqual(self.audit()["errors"], [])

    def test_conventional_instruction_names_are_preserved(self):
        for name in ("AGENTS.md", "CLAUDE.md", "GEMINI.md"):
            self.add("docs/reference/mcp/" + name, "# Instructions\n")
        self.assertEqual(self.audit()["errors"], [])

    def test_ordinary_uppercase_and_underscores_fail(self):
        for name in ("docs/guides/SETUP.md", "docs/guides/getting_started.md"):
            with self.subTest(name=name):
                self.add(name, "# Guide\n")
                self.assertIn("filename-casing", self.kinds())

    def test_case_collisions_fail_even_on_case_insensitive_hosts(self):
        self.add("docs/guides/setup.md", "# Guide\n")
        self.add("docs/guides/Setup.md", "# Guide\n")
        self.assertIn("case-collision", self.kinds())

    def test_directory_case_is_checked(self):
        self.add("docs/guides/MCP/guide.md", "# Guide\n")
        self.assertIn("directory-casing", self.kinds())

    def test_loose_root_guides_fail(self):
        self.add("docs/setup.md", "# Guide\n")
        self.assertIn("flat-root-document", self.kinds())

    def test_unknown_sections_fail(self):
        self.add("docs/random/guide.md", "# Guide\n")
        self.assertIn("unknown-section", self.kinds())

    def test_empty_current_docs_fail(self):
        self.add("docs/guides/empty.md", "")
        self.assertIn("empty-current-document", self.kinds())

    def test_empty_historical_artifacts_are_visible_not_deleted(self):
        target = self.add("docs/history/raycast/placeholder.sh", "")
        result = self.audit()
        self.assertEqual(result["errors"], [])
        self.assertEqual(result["empty_historical_artifacts"], ["docs/history/raycast/placeholder.sh"])
        self.assertTrue(target.is_file())


class LinkTests(Fixture):
    def test_valid_relative_links_and_anchors(self):
        self.add("docs/reference/api.md", "# API\n\n## Error codes\n")
        self.add("docs/guides/setup.md", "# Setup\n[API](../reference/api.md#error-codes)\n")
        self.assertEqual(self.audit()["errors"], [])
        self.assertEqual(self.audit()["local_links"], 1)

    def test_multiline_link_labels_are_checked(self):
        self.add("docs/reference/api.md", "# API\n## Errors\n")
        self.add("docs/guides/setup.md", "# Setup\n[API reference\nand errors](../reference/api.md#errors)\n")
        self.assertEqual(self.audit()["errors"], [])
        self.assertEqual(self.audit()["local_links"], 1)

    def test_broken_multiline_links_are_not_silently_skipped(self):
        self.add("docs/guides/setup.md", "# Setup\n[Old setup\ncontract](SETUP.md#installation)\n")
        self.assertIn("missing-target", self.kinds())

    def test_missing_path_fails(self):
        self.add("docs/guides/setup.md", "# Setup\n[Missing](missing.md)\n")
        self.assertIn("missing-target", self.kinds())

    def test_wrong_case_fails_independently_of_host_filesystem(self):
        self.add("docs/reference/api.md", "# API\n")
        self.add("docs/guides/setup.md", "# Setup\n[API](../reference/API.md)\n")
        self.assertIn("missing-target", self.kinds())

    def test_missing_anchor_fails(self):
        self.add("docs/reference/api.md", "# API\n[Missing](#not-here)\n")
        self.assertIn("missing-anchor", self.kinds())

    def test_duplicate_heading_anchors_are_supported(self):
        self.add("docs/reference/api.md", "# API\n## Errors\n## Errors\n[Second](#errors-1)\n")
        self.assertEqual(self.audit()["errors"], [])

    def test_code_examples_comments_and_inline_code_are_not_links(self):
        fence = chr(96) * 3
        content = "# Guide\n" + fence + "md\n[Example](missing.md)\n" + fence + "\n"
        content += "<!--\n[Comment](missing.md)\n-->\n" + chr(96) + "[Code](missing.md)" + chr(96) + "\n"
        self.add("docs/guides/guide.md", content)
        self.assertEqual(self.audit()["errors"], [])
        self.assertEqual(len(docs.prose(content)), len(content))
        self.assertEqual(docs.prose(content).count("\n"), content.count("\n"))

    def test_reference_style_and_html_links_are_checked(self):
        self.add("docs/guides/guide.md", '# Guide\n[ref]: absent.md\n<a href="also-absent.md">Link</a>\n')
        self.assertEqual(len(self.audit()["errors"]), 2)

    def test_explicit_html_anchor_is_supported(self):
        self.add("docs/guides/guide.md", '# Guide\n<a id="stable"></a>\n[Stable](#stable)\n')
        self.assertEqual(self.audit()["errors"], [])

    def test_external_links_are_reported_as_unfetched(self):
        self.add("docs/guides/guide.md", "# Guide\n[External](https://example.com/docs)\n")
        report = self.audit()
        self.assertEqual(report["errors"], [])
        self.assertEqual(report["external_links_not_fetched"], 1)

    def test_package_readme_mirror_uses_root_link_semantics(self):
        self.add("docs/guides/guide.md", "# Guide\n")
        self.add("README.md", "# Project\n[Guide](docs/guides/guide.md)\n")
        self.add("packages/cortex-rmcp/README.md", (self.root / "README.md").read_text())
        self.assertEqual(self.audit()["errors"], [])

    def test_retired_doc_paths_are_caught_from_other_packages(self):
        self.add("plugins/demo/README.md", "# Plugin\n[Guide](../../docs/SETUP.md)\n")
        self.assertIn("missing-target", self.kinds())


class IndexTests(Fixture):
    def setUp(self):
        super().setUp()
        self.add("docs/reference/mcp/tools.md", "# MCP tools\n")
        self.add("docs/guides/setup.md", "# Setup\n")

    def test_check_does_not_create_missing_indexes(self):
        issues = self.indexes(True)
        self.assertTrue(issues)
        self.assertFalse((self.root / "docs/reference/README.md").exists())

    def test_generation_is_deterministic_and_idempotent(self):
        expected = docs.index_contents(self.root, self.files)
        self.assertEqual(expected, docs.index_contents(self.root, list(reversed(self.files))))
        self.assertEqual(self.indexes(False), [])
        path = self.root / "docs/reference/README.md"
        before = path.read_bytes(), path.stat().st_mtime_ns
        self.assertEqual(self.indexes(False), [])
        self.assertEqual((path.read_bytes(), path.stat().st_mtime_ns), before)
        self.assertEqual(self.indexes(True), [])

    def test_new_documents_are_discovered_without_a_manifest(self):
        self.indexes(False)
        self.add("docs/reference/mcp/prompts.md", "# MCP prompts\n")
        self.assertTrue(self.indexes(True))
        self.indexes(False)
        self.assertIn("prompts.md", (self.root / "docs/reference/mcp/README.md").read_text())

    def test_authored_index_is_never_overwritten(self):
        path = self.add("docs/reference/README.md", "# Authored index\nKeep me.\n")
        original = path.read_bytes()
        self.assertTrue(any("authored index" in issue for issue in self.indexes(False)))
        self.assertEqual(path.read_bytes(), original)

    def test_root_home_page_remains_authored(self):
        original = (self.root / "docs/README.md").read_bytes()
        self.indexes(False)
        self.assertEqual((self.root / "docs/README.md").read_bytes(), original)

    def test_symlink_index_is_rejected(self):
        path = self.root / "docs/reference/README.md"
        path.symlink_to(self.root / "README.md")
        self.assertTrue(any("symlink" in issue for issue in self.indexes(False)))
        self.assertTrue(path.is_symlink())
        self.assertEqual((self.root / "README.md").read_text(), "# Project\n")

    def test_orphan_generated_index_is_not_silently_deleted(self):
        target = self.add("docs/history/abandoned/README.md", docs.MARKER + "\n# Old\n")
        self.assertTrue(any("orphan" in issue for issue in self.indexes(False)))
        self.assertTrue(target.is_file())

    def test_all_generated_indexes_have_resolving_links(self):
        self.indexes(False)
        paths = [str(path.relative_to(self.root)) for path in self.root.rglob("*.md")]
        self.assertEqual(docs.audit(self.root, paths)["errors"], [])

    def test_history_indexes_explicitly_label_empty_placeholders(self):
        self.add("docs/history/raycast/placeholder.sh", "")
        self.indexes(False)
        self.assertIn("not a runnable integration", (self.root / "docs/history/raycast/README.md").read_text())


class DiscoveryTests(Fixture):
    def test_git_discovery_ignores_caches_and_includes_new_docs(self):
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        self.add(".gitignore", ".cache/\n")
        self.add(".cache/private.md", "# Private\n")
        self.add("docs/guides/new.md", "# New\n")
        found = docs.tracked_files(self.root)
        self.assertIn("docs/guides/new.md", found)
        self.assertNotIn(".cache/private.md", found)

    def test_docs_gate_runs_both_regression_suites(self):
        justfile = (ROOT / "Justfile").read_text()
        ci = (ROOT / "tests/live/selftest/ci-docs.sh").read_text()
        for script in ("test-generated-docs.py", "test-documentation.py"):
            self.assertIn(script, justfile)
            self.assertIn(script, ci)


if __name__ == "__main__":
    unittest.main()
