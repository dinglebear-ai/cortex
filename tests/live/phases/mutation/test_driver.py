#!/usr/bin/env python3
import json
import os
import signal
import time
import pathlib
import subprocess
import sys
import tempfile
import unittest

DRIVER = pathlib.Path(__file__).with_name('run.py')
CHECK = '''from value import value
ok = value == 1
print('test fixture::value ... ' + ('ok' if ok else 'FAILED'))
print('test result: ' + ('ok. 1 passed; 0 failed;' if ok else 'FAILED. 0 passed; 1 failed;'))
raise SystemExit(0 if ok else 101)
'''

class MutationTests(unittest.TestCase):
    def run_case(self, replacement, killer=None, mapped=True):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / 'source'
            source.mkdir()
            (source / 'value.py').write_text('value = 1\n')
            (source / 'build.py').write_text('import py_compile\npy_compile.compile("value.py", doraise=True)\n')
            (source / 'check.py').write_text(killer or CHECK)
            manifest = root / 'manifest.json'
            manifest.write_text(json.dumps({'mutants': [{'id': 'fixture', 'fingerprint': 'value',
                'killer': 'literal assertion', 'expected_test': 'fixture::value' if mapped else None,
                'target': 'value.py', 'needle': 'value = 1', 'replacement': replacement}]}))
            command = [sys.executable, str(DRIVER), '--source', str(source), '--workspace',
                str(root / 'mutated'), '--manifest', str(manifest), '--build', sys.executable,
                'build.py', '--killer', sys.executable, 'check.py']
            result = subprocess.run(command, capture_output=True, text=True,
                env={**os.environ, 'CARGO_TARGET_DIR': str(root / 'shared-target'),
                     'CARGO_BUILD_BUILD_DIR': str(root / 'shared-build')})
            self.assertEqual((source / 'value.py').read_text(), 'value = 1\n')
            return result.returncode, json.loads(result.stdout)

    def assert_status(self, replacement, status, killer=None, mapped=True):
        code, report = self.run_case(replacement, killer, mapped)
        self.assertEqual(code, 0 if status == 'killed' else 1)
        self.assertEqual(report['results'][0]['status'], status)

    def test_assertion_kills_compiled_mutant(self):
        self.assert_status('value = 2', 'killed')

    def test_build_cache_is_owned_by_disposable_workspace(self):
        check = ("import os, pathlib\n"
                 "assert pathlib.Path(os.environ['CARGO_TARGET_DIR']) == pathlib.Path.cwd() / '.mutation-target'\n"
                 "assert pathlib.Path(os.environ['CARGO_BUILD_BUILD_DIR']) == pathlib.Path.cwd() / '.mutation-build'\n") + CHECK
        code, report = self.run_case('value = 2', check)
        self.assertEqual(code, 0)
        self.assertEqual(report['results'][0]['status'], 'killed')
        self.assertTrue(report['cargo_target_dir'].endswith('/mutated/.mutation-target'))

    def test_interruption_reaps_child_and_restores_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / 'source'
            source.mkdir()
            (source / 'value.py').write_text('value = 1\n')
            (source / 'check.py').write_text(CHECK)
            (source / 'build.py').write_text(
                "import os, pathlib, time\n"
                "if os.environ['MUTANT_ID'] != 'baseline':\n"
                " pathlib.Path('child.pid').write_text(str(os.getpid()))\n"
                " time.sleep(60)\n")
            manifest = root / 'manifest.json'
            manifest.write_text(json.dumps({'mutants': [{'id': 'fixture', 'fingerprint': 'value',
                'killer': 'literal assertion', 'expected_test': 'fixture::value',
                'target': 'value.py', 'needle': 'value = 1', 'replacement': 'value = 2'}]}))
            tree = root / 'tree'
            process = subprocess.Popen([sys.executable, str(DRIVER), '--source', str(source),
                '--workspace', str(tree), '--manifest', str(manifest), '--build', sys.executable,
                'build.py', '--killer', sys.executable, 'check.py'],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            child = None
            try:
                deadline = time.monotonic() + 10
                while not (tree / 'child.pid').exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue((tree / 'child.pid').exists())
                child = int((tree / 'child.pid').read_text())
                process.send_signal(signal.SIGTERM)
                self.assertEqual(process.wait(timeout=5), 143)
                with self.assertRaises(ProcessLookupError):
                    os.kill(child, 0)
                self.assertEqual((tree / 'value.py').read_text(), 'value = 1\n')
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                if child:
                    try:
                        os.killpg(child, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_compile_failure_is_invalid(self):
        self.assert_status('value = (', 'invalid')

    def test_unconditional_failure_rejects_baseline(self):
        self.assert_status('value = 2', 'baseline-failed', 'raise SystemExit(42)\n')

    def test_survivor_fails_qualification(self):
        self.assert_status('value = 1 + 0', 'survived')

    def test_launch_failure_after_baseline_is_not_a_kill(self):
        check = "import os\nif os.environ['MUTANT_ID'] != 'baseline': raise SystemExit(101)\n" + CHECK
        self.assert_status('value = 2', 'harness-error', check)

    def test_unrelated_failed_test_is_not_a_kill(self):
        check = "import os\nif os.environ['MUTANT_ID'] != 'baseline':\n print('test unrelated::test ... FAILED\\ntest result: FAILED. 0 passed; 1 failed;'); raise SystemExit(101)\n" + CHECK
        self.assert_status('value = 2', 'harness-error', check)

    def test_missing_test_does_not_pass_baseline(self):
        self.assert_status('value = 2', 'baseline-failed', "print('test result: ok. 0 passed; 0 failed;')\n")

    def test_unmapped_mutation_is_unqualified(self):
        self.assert_status('value = 2', 'unqualified', mapped=False)

if __name__ == '__main__':
    unittest.main()
