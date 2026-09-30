#!/usr/bin/env python3
"""Hermetic regressions for generated documentation and its CI routing."""
import importlib.util
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


generator = load("generated_docs", "scripts/generate-docs.py")
live = load("live_docs", "tests/live/generate-docs.py")
router = load("ci_paths", "scripts/ci/changed_paths.py")


class SchemaTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "contracts").mkdir()
        self.source = self.root / "contracts/one.schema.json"
        self.source.write_bytes(b'{"type": "object"}\n')
        self.target = self.root / "docs/reference/contracts/generated/one.schema.json"

    def test_check_missing_snapshot_does_not_create_it(self):
        self.assertFalse(generator.sync_schemas(self.root, True))
        self.assertFalse(self.target.exists())

    def test_source_bytes_are_copied_and_generation_is_idempotent(self):
        self.assertTrue(generator.sync_schemas(self.root, False))
        self.assertEqual(self.target.read_bytes(), self.source.read_bytes())
        before = self.target.stat().st_mtime_ns
        self.assertTrue(generator.sync_schemas(self.root, False))
        self.assertEqual(before, self.target.stat().st_mtime_ns)
        self.assertTrue(generator.sync_schemas(self.root, True))

    def test_check_drift_is_read_only(self):
        generator.sync_schemas(self.root, False)
        self.target.write_bytes(b'{}\n')
        before = self.target.stat().st_mtime_ns
        self.assertFalse(generator.sync_schemas(self.root, True))
        self.assertEqual(self.target.read_bytes(), b'{}\n')
        self.assertEqual(before, self.target.stat().st_mtime_ns)

    def test_new_schema_is_discovered(self):
        (self.root / "contracts/two.schema.json").write_text('{}\n')
        generator.sync_schemas(self.root, False)
        self.assertTrue(self.target.with_name("two.schema.json").is_file())

    def test_all_sources_are_validated_before_any_write(self):
        (self.root / "contracts/two.schema.json").write_text('invalid')
        with self.assertRaises(ValueError):
            generator.sync_schemas(self.root, False)
        self.assertFalse(self.target.exists())

    def test_orphans_fail_without_deletion(self):
        generator.sync_schemas(self.root, False)
        orphan = self.target.with_name("orphan.schema.json")
        orphan.write_text('{}')
        with self.assertRaisesRegex(ValueError, "without canonical"):
            generator.sync_schemas(self.root, False)
        self.assertTrue(orphan.exists())

    def test_symlink_cannot_overwrite_canonical_source(self):
        self.target.parent.mkdir(parents=True)
        self.target.symlink_to(self.source)
        with self.assertRaisesRegex(ValueError, "regular file"):
            generator.sync_schemas(self.root, False)
        self.assertEqual(self.source.read_bytes(), b'{"type": "object"}\n')

    def test_empty_source_directory_is_not_a_vacuous_pass(self):
        self.source.unlink()
        with self.assertRaises(ValueError):
            generator.sync_schemas(self.root, True)


class InventoryTests(unittest.TestCase):
    def setUp(self):
        self.contract = {"entries": [{"kind": kind} for kind in live.KINDS]}
        self.generated = live.render_inventory(self.contract, {"zeta": {}, "alpha": {}})
        self.text = "---\ntitle: Maintained\n---\n" + live.BEGIN + "\nold\n" + live.END + "\nHandwritten suffix.\n"

    def test_counts_and_profiles_come_from_inputs(self):
        self.assertIn("| all surfaces | 6 |", self.generated)
        self.assertIn("| runnable profiles | 2 |", self.generated)
        self.assertIn("Profiles: `alpha`, `zeta`", self.generated)
        for kind in live.KINDS:
            self.assertIn(f"| {kind} surfaces | 1 |", self.generated)

    def test_profile_order_is_deterministic(self):
        self.assertEqual(self.generated, live.render_inventory(self.contract, {"alpha": {}, "zeta": {}}))

    def test_unknown_surface_kind_requires_documentation(self):
        with self.assertRaisesRegex(ValueError, "undocumented surface"):
            live.render_inventory({"entries": [{"kind": "new-kind"}]}, {})

    def test_maintained_prose_and_newlines_survive(self):
        self.text = self.text.replace(chr(10), chr(13) + chr(10))
        updated = live.replace_inventory(self.text, self.generated)
        self.assertTrue(updated.startswith(self.text.split(live.BEGIN)[0]))
        self.assertTrue(updated.endswith(self.text.split(live.END)[1]))
        self.assertEqual(updated, live.replace_inventory(updated, self.generated))

    def test_damaged_markers_fail_in_both_modes_without_writes(self):
        cases = ["no markers", live.BEGIN, live.END, live.END + live.BEGIN,
                 live.BEGIN + live.BEGIN + live.END, live.BEGIN + live.END + live.END]
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "doc.md"
            for text in cases:
                for check in (True, False):
                    with self.subTest(text=text, check=check):
                        target.write_bytes(text.encode())
                        with self.assertRaises(ValueError):
                            live.update_file(target, self.generated, check)
                        self.assertEqual(target.read_bytes(), text.encode())

    def test_check_reports_drift_then_repair_is_idempotent(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "doc.md"
            target.write_bytes(self.text.encode())
            self.assertFalse(live.update_file(target, self.generated, True))
            self.assertEqual(target.read_bytes(), self.text.encode())
            self.assertTrue(live.update_file(target, self.generated, False))
            before = target.stat().st_mtime_ns
            self.assertTrue(live.update_file(target, self.generated, False))
            self.assertEqual(before, target.stat().st_mtime_ns)
            self.assertTrue(live.update_file(target, self.generated, True))

    def test_exporter_is_locked_and_uses_repo_working_directory(self):
        with patch.object(live.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, '{"entries": []}')) as run:
            self.assertEqual(live.load_contract(ROOT), {"entries": []})
        self.assertIn("--locked", run.call_args.args[0])
        self.assertEqual(run.call_args.kwargs["cwd"], ROOT)
        self.assertTrue(run.call_args.kwargs["check"])


class PackageMirrorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.package = self.root / "packages/cortex-rmcp"
        (self.package / "scripts").mkdir(parents=True)
        self.script = self.package / "scripts/sync-readme.js"
        shutil.copyfile(ROOT / "packages/cortex-rmcp/scripts/sync-readme.js", self.script)
        (self.root / "README.md").write_bytes(b'# Root\n')
        (self.root / "LICENSE").write_bytes(b'License\n')

    def run_script(self, *args):
        return subprocess.run(["node", str(self.script), *args], capture_output=True, text=True)

    def test_missing_mirrors_fail_check_without_creation(self):
        self.assertNotEqual(self.run_script("--check").returncode, 0)
        self.assertFalse((self.package / "README.md").exists())

    def test_write_and_check_are_byte_identical_and_idempotent(self):
        self.assertEqual(self.run_script().returncode, 0)
        for name in ("README.md", "LICENSE"):
            self.assertEqual((self.root / name).read_bytes(), (self.package / name).read_bytes())
        before = (self.package / "README.md").stat().st_mtime_ns
        self.assertEqual(self.run_script().returncode, 0)
        self.assertEqual(before, (self.package / "README.md").stat().st_mtime_ns)
        self.assertEqual(self.run_script("--check").returncode, 0)

    def test_license_drift_is_checked_without_repair(self):
        self.run_script()
        (self.package / "LICENSE").write_text('stale')
        self.assertNotEqual(self.run_script("--check").returncode, 0)
        self.assertEqual((self.package / "LICENSE").read_text(), 'stale')

    def test_check_preserves_valid_and_broken_symlinks(self):
        target = self.package / "README.md"
        for source in (self.root / "README.md", self.root / "missing"):
            with self.subTest(source=source):
                target.symlink_to(source)
                self.assertNotEqual(self.run_script("--check").returncode, 0)
                self.assertTrue(target.is_symlink())
                self.assertEqual(self.run_script().returncode, 0)
                self.assertFalse(target.is_symlink())
                target.unlink()

    def test_unknown_flag_fails_without_writing(self):
        self.assertNotEqual(self.run_script("--unknown").returncode, 0)
        self.assertFalse((self.package / "README.md").exists())


class EntryPointTests(unittest.TestCase):
    def test_check_delegates_to_existing_owners_and_propagates_failure(self):
        with patch.object(generator, "sync_schemas", return_value=True), patch.object(generator.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)) as run:
            self.assertEqual(generator.generate(ROOT, True), 1)
        calls = [call.args[0] for call in run.call_args_list]
        self.assertEqual(len(calls), 4)
        self.assertIn("--check", calls[0])
        self.assertIn("--check", calls[1])
        self.assertTrue(calls[2][-1].endswith("check-integration-contracts.py"))
        self.assertTrue(calls[3][1].endswith("documentation.py"))
        self.assertIn("--check", calls[3])

    def test_generator_inputs_and_outputs_route_to_documentation_ci(self):
        for path in ("contracts/new.schema.json", "tests/TEST_COVERAGE.md", "LICENSE", "Justfile",
                     "packages/cortex-rmcp/README.md", "packages/cortex-rmcp/scripts/sync-readme.js"):
            with self.subTest(path=path):
                self.assertTrue(router.classify("pull_request", [path])["docs"])

    def test_both_rust_and_docs_only_ci_run_generator_checks(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        test_job = workflow.split("\n  test:", 1)[1].split("\n  session-query-benchmark:", 1)[0]
        docs_job = workflow.split("\n  docs-contract:", 1)[1].split("\n  coverage:", 1)[0]
        for job in (test_job, docs_job):
            self.assertIn("bash tests/live/selftest/ci-docs.sh", job)


if __name__ == "__main__":
    unittest.main()
