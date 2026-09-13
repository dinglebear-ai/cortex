#!/usr/bin/env python3
"""Failure-path checks for repository review corrections; no live services."""
import ast
import importlib.util
import io
import json
import os
import pathlib
import re
import subprocess
import sqlite3
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
def load(relative):
    spec = importlib.util.spec_from_file_location('review_module', ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

class ReviewRegressions(unittest.TestCase):
    def test_restore_failed_copy_preserves_database_and_wal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory); backup = root / 'backups'; data = root / 'data'
            backup.mkdir(); data.mkdir()
            for name in ['cortex.db', 'cortex.db-wal', 'auth.db', 'auth.db-wal']:
                (data / name).write_text('original')
            command = ['sh', str(ROOT / 'scripts/restore-backup.sh'), str(backup), 'test', str(data)]
            self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
            with sqlite3.connect(backup / 'syslog-test.db') as conn:
                conn.execute('CREATE TABLE expected(value TEXT)')
                conn.execute("INSERT INTO expected VALUES ('replacement')")
            # An existing unreadable-as-file optional backup causes staging failure.
            (backup / 'auth-test.db').mkdir()
            self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
            for name in ['cortex.db', 'cortex.db-wal', 'auth.db', 'auth.db-wal']:
                self.assertEqual((data / name).read_text(), 'original')
            (backup / 'auth-test.db').rmdir()
            saved = (backup / 'syslog-test.db').read_bytes()
            for corrupt in [b'', b'not a SQLite database']:
                (backup / 'syslog-test.db').write_bytes(corrupt)
                self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
                self.assertEqual((data / 'cortex.db-wal').read_text(), 'original')
                self.assertEqual((data / 'cortex.db').read_text(), 'original')
            (backup / 'syslog-test.db').write_bytes(saved)
            subprocess.run(command, check=True)
            with sqlite3.connect(data / 'cortex.db') as conn:
                self.assertEqual(conn.execute('SELECT value FROM expected').fetchone()[0], 'replacement')
            self.assertFalse((data / 'cortex.db-wal').exists())
            self.assertEqual((data / 'auth.db-wal').read_text(), 'original')

    def test_redaction_bounded_lines_and_stream(self):
        module = load('tests/live/lib/redact.py'); out = io.StringIO()
        module.redact_stream(io.BytesIO(b'prefix sensitive-value\nAuthorization: Bearer abc\n'), out, ['sensitive-value'], 100)
        self.assertEqual(out.getvalue(), 'prefix [REDACTED]\nAuthorization: Bearer [REDACTED]\n')
        out = io.StringIO()
        with self.assertRaises(ValueError):
            module.redact_stream(io.BytesIO(b'x' * (module.MAX_LINE + 1)), out, [], 2 * module.MAX_LINE)
        self.assertEqual(out.getvalue(), '')
        with self.assertRaises(ValueError):
            module.redact_stream(io.BytesIO(b'1234\n5678\n'), io.StringIO(), [], 6)

    def test_docker_memory_units(self):
        tree = ast.parse((ROOT / 'tests/live/phases/telemetry/collector.py').read_text())
        function = next(node for node in ast.walk(tree) if isinstance(node, ast.FunctionDef) and node.name == 'size')
        namespace = {'units': {'B': 1, 'KiB': 1024, 'MiB': 1048576, 'GiB': 1073741824}}
        exec(compile(ast.Module(body=[function], type_ignores=[]), '<actual collector size>', 'exec'), namespace)
        for value, expected in [('12B', 12), ('1KiB', 1024), ('1.5MiB', 1572864), ('2GiB', 2147483648)]:
            self.assertEqual(namespace['size'](value), expected)

    def test_stale_upgrade_matrix_is_rejected(self):
        module = load('tests/live/phases/upgrade/validate.py'); module.validate(ROOT)
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory); dest = root / 'tests/live/contracts/releases'; dest.mkdir(parents=True)
            (root / 'Cargo.toml').write_text('[package]\nversion="9.0.0"\n')
            (dest / 'compatibility.json').write_text((ROOT / 'tests/live/contracts/releases/compatibility.json').read_text())
            with self.assertRaisesRegex(ValueError, 'does not match'):
                module.validate(root)

    def test_deployment_retry_uses_successful_revision_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory); tools = root / 'bin'; tools.mkdir()
            (root / '.git').mkdir(); (root / 'scripts').mkdir()
            (root / 'Cargo.toml').write_text('[package]\nversion = "3.16.0"\n')
            (root / 'scripts/prepare-compose-dirs.sh').write_text('exit 0\n')
            commands = {
                'git': 'case "$*" in "branch --show-current") echo main;; "rev-parse HEAD") echo revision-two;; "rev-parse --git-path cortex-deployed-revision") echo .git/receipt;; esac\n',
                'flock': 'exit 0\n',
                'curl': 'exit 0\n',
                'docker': 'case "$*" in "exec cortex cortex --version") echo "cortex 3.16.0";; "compose build cortex") echo build >>"$BUILD_LOG"; test "$FAIL_BUILD" = 0;; esac\n',
            }
            for name, body in commands.items():
                path = tools / name; path.write_text('#!/bin/sh\n' + body); path.chmod(0o755)
            env = {**os.environ, 'PATH': str(tools) + os.pathsep + os.environ['PATH'], 'CORTEX_AUTO_DEPLOY_REPO': str(root), 'CORTEX_AUTO_DEPLOY_LOCK': str(root/'lock'), 'BUILD_LOG': str(root/'builds'), 'FAIL_BUILD': '1'}
            command = ['bash', str(ROOT/'scripts/auto-deploy.sh')]
            self.assertNotEqual(subprocess.run(command, env=env, capture_output=True).returncode, 0)
            self.assertFalse((root/'.git/receipt').exists())
            env['FAIL_BUILD'] = '0'
            subprocess.run(command, env=env, capture_output=True, check=True)
            subprocess.run(command, env=env, capture_output=True, check=True)
            self.assertEqual((root/'builds').read_text(), 'build\nbuild\n')
            self.assertEqual((root/'.git/receipt').read_text(), 'revision-two\n')

    def test_signed_cursor_wire_shape(self):
        schema = json.loads((ROOT / 'contracts/rendered-session-page.schema.json').read_text())
        pattern = schema['properties']['next_cursor']['pattern']
        self.assertIsNotNone(re.fullmatch(pattern, '7b2276657273696f6e223a317d'))
        self.assertIsNone(re.fullmatch(pattern, 'cortex-session-v1:12'))
        self.assertIsNone(re.fullmatch(pattern, 'abc'))

if __name__ == '__main__':
    unittest.main()
