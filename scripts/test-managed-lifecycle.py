#!/usr/bin/env python3
"""Owned-target recovery/scheduler fixtures. Never contact a Docker daemon."""
import hashlib
import json
import os
import re
from pathlib import Path
import sqlite3
import subprocess
import tarfile
import tempfile
import unittest

HELPER = Path(__file__).with_name("managed-lifecycle.sh").resolve()
FAKE_DOCKER = r'''#!/usr/bin/env python3
import hashlib, json, os, pathlib, sqlite3, sys, tarfile
home = pathlib.Path(os.environ['TEST_HOME'])
root = home / 'backups'
data = home / 'data'
args = sys.argv[1:]
with (home / 'docker-calls.jsonl').open('a') as log:
    log.write(json.dumps(args) + '\n')
if args[0] == 'info':
    sys.exit(int(os.environ.get('FAIL_DAEMON', '0')))
if args[0] == 'inspect':
    if os.environ.get('MISSING_CONTAINER') == '1': sys.exit(1)
    if '-f' not in args: sys.exit(0)
    template = args[args.index('-f') + 1]
    if '{{if .RW}}' in template:
        print('/data|bind|' + str(data))
        print('/backups|bind|' + str(root))
        print('/cortex-home|bind|' + str(home))
        if os.environ.get('EXTRA_WRITABLE_MOUNT'): print('/external-auth|bind|/external/auth')
    elif 'config_files' in template:
        print('/foreign/docker-compose.yml' if os.environ.get('FOREIGN_CONTAINER') else home / 'compose/docker-compose.yml')
    elif '.Destination "/backups"' in template: print(root)
    elif '.Destination "/data"' in template:
        print('bind|' + str(home / ('foreign-data' if os.environ.get('WRONG_MOUNT') else 'data')))
    elif 'com.docker.compose.service' in template: print('cortex')
    elif '.Image' in template: print('sha256:' + 'a' * 64)
    elif '.State.Running' in template: print('true')
    sys.exit(0)
if args[:2] == ['image', 'inspect']: sys.exit(0)
if args[0] != 'compose': sys.exit(64)
if 'run' not in args: sys.exit(0)
script = sys.stdin.read() if '-s' in args else ''
if '# Capture stopped data' in script:
    if os.environ.get('FAIL_SNAPSHOT'): sys.exit(73)
    stamp = args[-1]
    bundle = root / ('recovery-' + stamp)
    with tarfile.open(bundle / 'data.tar', 'w') as tar:
        for entry in data.iterdir(): tar.add(entry, arcname='./' + entry.name)
    with tarfile.open(bundle / 'home-auth.tar', 'w') as tar: pass
    # Fixture verifies real SQLite data, matching the copied-db verification
    # contract without executing any real containers.
    with sqlite3.connect(data / 'cortex.db') as db:
        assert db.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
    (bundle / 'primary-db').write_text('cortex.db\n')
    names = ['data.tar', 'home-auth.tar', 'config.tar', 'image', 'managed-home', 'primary-db', 'data-mount', 'writable-mounts']
    (bundle / 'SHA256SUMS').write_text(''.join(hashlib.sha256((bundle / n).read_bytes()).hexdigest() + '  ' + n + '\n' for n in names))
    (bundle / 'VERIFIED').write_text(stamp + '\n')
    sys.exit(0)
if '.before-restore.' in script:
    # Inject a failed restore staging operation. It must leave existing data
    # alone and return failure while the outer helper restarts that runtime.
    sys.exit(73 if os.environ.get('FAIL_RESTORE') else 0)
if '-c' in args and 'du -sk --exclude=' in args[args.index('-c') + 1]:
    print('1024'); print('0' if os.environ.get('LOW_SPACE') else '999999999'); sys.exit(0)
if '-c' in args:
    bundle = root / pathlib.Path(args[-1]).name
    for line in (bundle / 'SHA256SUMS').read_text().splitlines():
        expected, name = line.split('  ', 1)
        if hashlib.sha256((bundle / name).read_bytes()).hexdigest() != expected: sys.exit(1)
    sys.exit(0)
sys.exit(64)
'''
FAKE_CRONTAB = r'''#!/bin/sh
file=$TEST_HOME/crontab
if test "$1" = -l; then
  if test -f "$file"; then cat "$file"; else echo 'no crontab for fixture' >&2; exit 1; fi
else cp "$1" "$file"
fi
'''

class LifecycleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        for name in ['data', 'backups', 'compose', 'bin']:
            (self.home / name).mkdir()
        (self.home / '.env').write_text('CORTEX_API_TOKEN=fixture-secret\n')
        (self.home / 'compose/docker-compose.yml').write_text('services:\n  cortex:\n    image: fixture\n')
        (self.home / 'managed-lifecycle.sh').write_bytes(HELPER.read_bytes())
        with sqlite3.connect(self.home / 'data/cortex.db') as db:
            db.execute('CREATE TABLE proof (value TEXT)')
            db.execute("INSERT INTO proof VALUES ('retained-current')")
        for name, script in [('docker', FAKE_DOCKER), ('crontab', FAKE_CRONTAB)]:
            path = self.home / 'bin' / name
            path.write_text(script)
            path.chmod(0o700)
        self.env = dict(os.environ, TEST_HOME=str(self.home), CORTEX_BIN='/installed/cortex', PATH=str(self.home / 'bin') + ':' + os.environ['PATH'])

    def run_helper(self, action: str, stamp: str = '', **extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(['sh', str(HELPER), action, str(self.home), stamp], env=dict(self.env, **extra), capture_output=True, text=True, timeout=20)

    def calls(self) -> list[list[str]]:
        path = self.home / 'docker-calls.jsonl'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def snapshot(self) -> tuple[Path, str]:
        result = self.run_helper('create')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn('fixture-secret', result.stdout + result.stderr)
        bundle = next((self.home / 'backups').glob('recovery-*'))
        self.assertEqual(bundle.stat().st_mode & 0o777, 0o700)
        for artifact in bundle.iterdir(): self.assertEqual(artifact.stat().st_mode & 0o777, 0o600)
        return bundle, (bundle / 'VERIFIED').read_text().strip()

    def test_snapshot_stops_and_restarts_owned_service_and_verifies_bundle(self) -> None:
        bundle, stamp = self.snapshot()
        operations = self.calls()
        self.assertTrue(any('stop' in call for call in operations))
        self.assertTrue(any('start' in call for call in operations))
        self.assertEqual((bundle / 'data-mount').read_text().strip(), 'bind|' + str(self.home / 'data'))

    def test_foreign_target_and_unavailable_daemon_fail_before_stop(self) -> None:
        for extra in [{'FOREIGN_CONTAINER': '1'}, {'FAIL_DAEMON': '1'}]:
            result = self.run_helper('prepare', **extra)
            self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any('stop' in call for call in self.calls()))
        self.assertFalse(list((self.home / 'backups').glob('recovery-*')))

    def test_unknown_writable_mount_fails_before_service_stop(self) -> None:
        result = self.run_helper('create', EXTRA_WRITABLE_MOUNT='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('outside verified managed recovery coverage', result.stderr)
        self.assertFalse(any('stop' in call for call in self.calls()))

    def test_first_install_preflight_does_not_snapshot_or_stop(self) -> None:
        result = self.run_helper('prepare', MISSING_CONTAINER='1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any('stop' in call for call in self.calls()))

    def test_wrong_data_mount_and_corrupt_bundle_fail_before_stop(self) -> None:
        bundle, stamp = self.snapshot()
        (self.home / 'docker-calls.jsonl').unlink()
        result = self.run_helper('restore', stamp, WRONG_MOUNT='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('different data mount', result.stderr)
        self.assertFalse(any('stop' in call for call in self.calls()))
        (bundle / 'data.tar').write_text('corrupted')
        result = self.run_helper('rollback', stamp)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any('stop' in call for call in self.calls()))

    def test_failed_restore_staging_retains_original_data_and_restarts_runtime(self) -> None:
        bundle, stamp = self.snapshot()
        before = (self.home / 'data/cortex.db').read_bytes()
        (self.home / 'docker-calls.jsonl').unlink()
        result = self.run_helper('restore', stamp, FAIL_RESTORE='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.home / 'data/cortex.db').read_bytes(), before)
        self.assertTrue(any('start' in call for call in self.calls()))
        self.assertEqual((bundle / 'VERIFIED').read_text().strip(), stamp)

    def test_existing_lifecycle_lock_rejects_snapshot_without_stopping(self) -> None:
        lock = self.home / '.lifecycle-lock'
        lock.mkdir()
        (lock / 'pid').write_text('999999999\n')
        result = self.run_helper('create')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('another lifecycle operation', result.stderr)
        self.assertFalse(any('stop' in call for call in self.calls()))
        self.assertTrue(lock.exists())

    def test_low_space_rejects_before_service_stop(self) -> None:
        result = self.run_helper('create', LOW_SPACE='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('insufficient free space', result.stderr)
        self.assertFalse(any('stop' in call for call in self.calls()))

    def test_retention_preserves_newest_protected_and_last_good_on_failure(self) -> None:
        first, _ = self.snapshot()
        (first / 'PROTECTED').touch()
        second = self.run_helper('create', CORTEX_BACKUP_RETAIN_COUNT='2')
        self.assertEqual(second.returncode, 0, second.stderr)
        third = self.run_helper('create', CORTEX_BACKUP_RETAIN_COUNT='2')
        self.assertEqual(third.returncode, 0, third.stderr)
        bundles = list((self.home / 'backups').glob('recovery-*'))
        self.assertEqual(len(bundles), 3)
        self.assertTrue(first.exists())
        fourth = self.run_helper('create', CORTEX_BACKUP_RETAIN_COUNT='2')
        self.assertEqual(fourth.returncode, 0, fourth.stderr)
        self.assertEqual(len(list((self.home / 'backups').glob('recovery-*'))), 3)
        before = {p.name for p in (self.home / 'backups').glob('recovery-*')}
        failure = self.run_helper('create', LOW_SPACE='1', CORTEX_BACKUP_RETAIN_COUNT='1')
        self.assertNotEqual(failure.returncode, 0)
        self.assertEqual(before, {p.name for p in (self.home / 'backups').glob('recovery-*')})

    def test_retention_leaves_foreign_unknown_and_symlink_marked_bundles(self) -> None:
        backup_root = self.home / 'backups'
        candidates: list[Path] = []
        for name in ['foreign-verified', 'foreign-incomplete', 'unknown', 'symlink-home',
                     'symlink-verified', 'symlink-protected', 'own-abandoned']:
            candidate = backup_root / ('recovery-' + name)
            candidate.mkdir()
            candidates.append(candidate)
            if name != 'unknown':
                (candidate / 'managed-home').write_text(
                    ('/foreign/home' if name.startswith('foreign') else str(self.home)) + '\n')
            if name in ['foreign-verified', 'symlink-protected']:
                (candidate / 'VERIFIED').write_text(name + '\n')
            if name == 'symlink-home':
                (candidate / 'managed-home').unlink()
                (candidate / 'managed-home').symlink_to(self.home / 'missing-marker')
            if name == 'symlink-verified':
                (candidate / 'VERIFIED').symlink_to(self.home / 'missing-marker')
            if name == 'symlink-protected':
                (candidate / 'PROTECTED').symlink_to(self.home / 'missing-marker')
            os.utime(candidate, (1, 1))
        for _ in range(3):
            result = self.run_helper('create', CORTEX_BACKUP_RETAIN_COUNT='1')
            self.assertEqual(result.returncode, 0, result.stderr)
        for candidate in candidates[:-1]:
            self.assertTrue(candidate.exists(), 'retention removed foreign/uncertain bundle: ' + candidate.name)
        self.assertFalse(candidates[-1].exists(), 'owned old incomplete bundle was not cleaned')
        verified_own = [path for path in backup_root.glob('recovery-*')
                        if path.name not in {candidate.name for candidate in candidates}]
        self.assertEqual(len(verified_own), 1)

    def test_parent_lease_excludes_other_operations_and_survives_child_snapshot(self) -> None:
        lock = self.home / '.lifecycle-lock'
        lock.mkdir()
        (lock / 'token').write_text('parent-operation')
        (lock / 'pid').write_text('external-deployment')
        result = self.run_helper('prepare', CORTEX_LIFECYCLE_TOKEN='parent-operation')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(lock.exists(), 'snapshot released outer deployment ownership')
        blocked = self.run_helper('create')
        self.assertNotEqual(blocked.returncode, 0)
        self.assertIn('another lifecycle operation', blocked.stderr)

    def test_failed_snapshot_cleans_incomplete_bundle_preserving_last_good(self) -> None:
        first, _ = self.snapshot()
        result = self.run_helper('create', FAIL_SNAPSHOT='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list((self.home / 'backups').glob('recovery-*')), [first])
        self.assertTrue((first / 'VERIFIED').exists())

    def assert_configuration_replacement(self, fail: bool) -> None:
        block = HELPER.read_text().split('    config_stage=', 1)[1].split('    # Always restore', 1)[0]
        block = 'config_stage=' + block
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            home, source, bundle, tools = [root / name for name in ['home', 'source', 'bundle', 'tools']]
            for path in [home, source, bundle, tools]:
                path.mkdir()
            for path in [home / 'compose', source / 'compose']:
                path.mkdir()
            (home / '.env').write_text('current-env')
            (home / 'compose/docker-compose.yml').write_text('current-compose')
            (home / 'compose/docker-compose.override.yml').write_text('current-override')
            (home / 'config.toml').write_text('current-config')
            (source / '.env').write_text('snapshot-env')
            (source / 'compose/docker-compose.yml').write_text('snapshot-compose')
            with tarfile.open(bundle / 'config.tar', 'w') as archive:
                archive.add(source / '.env', arcname='.env')
                archive.add(source / 'compose', arcname='compose')
            if fail:
                program = tools / 'mv'
                program.write_text('#!/usr/bin/env python3\nimport os,sys\na=sys.argv[1:]\n'
                    + 'if ".restore-config." in a[0] and a[0].endswith("/compose"): sys.exit(73)\n'
                    + 'os.execv("/bin/mv", ["mv"]+a)\n')
                program.chmod(0o700)
            script = ('set -eu\nhome="' + str(home) + '"\nbundle="' + str(bundle)
                + '"\nlock_owned=false\ncleanup_lifecycle() { exit "$?"; }\n' + block)
            result = subprocess.run(['sh', '-s'], input=script, text=True, capture_output=True,
                env=dict(os.environ, PATH=str(tools) + ':' + os.environ['PATH']), timeout=20)
            if fail:
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual((home / '.env').read_text(), 'current-env')
                self.assertEqual((home / 'compose/docker-compose.override.yml').read_text(), 'current-override')
                self.assertEqual((home / 'config.toml').read_text(), 'current-config')
            else:
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((home / '.env').read_text(), 'snapshot-env')
                self.assertFalse((home / 'compose/docker-compose.override.yml').exists())
                self.assertFalse((home / 'config.toml').exists())

    def test_configuration_recovery_removes_files_absent_snapshot(self) -> None:
        self.assert_configuration_replacement(False)

    def test_configuration_recovery_compensates_failed_commit(self) -> None:
        self.assert_configuration_replacement(True)

    def assert_real_restore_contract(self, failure: str) -> None:
        """Execute the exact container restore body on disposable host paths.

        This verifies real SQLite files and compensation after filesystem errors,
        independent of the mocked Docker orchestration used by the other cases.
        """
        block = HELPER.read_text().split("<<'RESTORE'\n", 1)[1].split('\nRESTORE\n', 1)[0]
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            data, home = root / 'data', root / 'home'
            bundle, bin_dir = root / 'backups/recovery-fixture', root / 'bin'
            for path in [data, home, bundle, bin_dir]:
                path.mkdir(parents=True)

            def database(path: Path, value: str) -> None:
                with sqlite3.connect(path) as connection:
                    connection.execute('CREATE TABLE proof(value TEXT)')
                    connection.execute('INSERT INTO proof VALUES (?)', (value,))

            database(data / 'cortex.db', 'current')
            (home / 'auth-jwt.pem').write_text('current-key')
            before = (data / 'cortex.db').read_bytes()
            staging = root / 'archive'
            staging.mkdir()
            database(staging / 'cortex.db', 'backup')
            (staging / 'auth-jwt.pem').write_text('backup-key')
            database(staging / 'auth.db', 'backup-auth')
            # Model committed WAL left after a crash/forced stop. The snapshot
            # predates these writes and intentionally contains no sidecars.
            (home / 'auth.db').write_bytes((staging / 'auth.db').read_bytes())
            connection = sqlite3.connect(home / 'auth.db')
            connection.execute('PRAGMA journal_mode=WAL')
            connection.execute('PRAGMA wal_autocheckpoint=0')
            connection.execute("UPDATE proof SET value='current-auth'")
            connection.commit()
            sidecars = {suffix: (home / ('auth.db' + suffix)).read_bytes() for suffix in ['-wal', '-shm']}
            connection.close()
            for suffix, contents in sidecars.items():
                (home / ('auth.db' + suffix)).write_bytes(contents)
            for archive, name in [('data.tar', 'cortex.db'), ('home-auth.tar', 'auth-jwt.pem')]:
                with tarfile.open(bundle / archive, 'w') as tar:
                    tar.add(staging / name, arcname='./' + name)
                    if archive == 'home-auth.tar':
                        tar.add(staging / 'auth.db', arcname='./auth.db')
            (bundle / 'primary-db').write_text('cortex.db\n')
            (bundle / 'SHA256SUMS').write_text(''.join(
                hashlib.sha256((bundle / name).read_bytes()).hexdigest() + '  ' + name + '\n'
                for name in ['data.tar', 'home-auth.tar', 'primary-db']))
            if failure != 'none':
                program = bin_dir / failure
                needle = '.restore-auth.' if failure == 'cp' else '.restore.'
                target = home / 'auth-jwt.pem' if failure == 'cp' else data
                program.write_text(
                    '#!/usr/bin/env python3\nimport os,sys\na=sys.argv[1:]\n'
                    + 'if any(' + repr(needle) + ' in v for v in a[:-1]) and a[-1].rstrip("/") == '
                    + repr(str(target)) + ': sys.exit(73)\n'
                    + 'os.execv(' + repr('/bin/' + failure) + ', [' + repr(failure) + ']+a)\n')
                program.chmod(0o700)
            mapping = {'data': str(data), 'backups': str(root / 'backups'), 'cortex-home': str(home)}
            # Match absolute roots only, never the data.tar filename in a bundle.
            script = re.sub(r'/(data|backups|cortex-home)(?=[/"\s])', lambda match: mapping[match[1]], block)
            result = subprocess.run(['sh', '-s', '--', 'fixture'], input=script, text=True,
                capture_output=True, env=dict(os.environ, PATH=str(bin_dir) + ':' + os.environ['PATH']), timeout=20)
            if failure == 'none':
                self.assertEqual(result.returncode, 0, result.stderr)
                with sqlite3.connect(data / 'cortex.db') as connection:
                    self.assertEqual(connection.execute('SELECT value FROM proof').fetchone()[0], 'backup')
                self.assertEqual((home / 'auth-jwt.pem').read_text(), 'backup-key')
                self.assertFalse((home / 'auth.db-wal').exists())
                with sqlite3.connect(home / 'auth.db') as connection:
                    self.assertEqual(connection.execute('SELECT value FROM proof').fetchone()[0], 'backup-auth')
            else:
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual((data / 'cortex.db').read_bytes(), before, 'original database was altered')
                self.assertEqual((home / 'auth-jwt.pem').read_text(), 'current-key', 'original key was altered')
                with sqlite3.connect(home / 'auth.db') as connection:
                    self.assertEqual(connection.execute('SELECT value FROM proof').fetchone()[0], 'current-auth')

    def test_real_restore_applies_sqlite_and_auth_key_snapshot(self) -> None:
        self.assert_real_restore_contract('none')

    def test_real_restore_compensates_primary_database_move_failure(self) -> None:
        self.assert_real_restore_contract('mv')

    def test_real_restore_compensates_auth_key_copy_failure(self) -> None:
        self.assert_real_restore_contract('cp')

    def test_container_verification_preserves_quoted_credentials(self) -> None:
        source = HELPER.parents[1] / 'src/deploy/verification.rs'
        block = source.read_text().split("<<'__CORTEX_VERIFY__'\n", 1)[1].split('\n__CORTEX_VERIFY__', 1)[0]
        # Rust format! doubles literal braces; execute the exact emitted shell.
        block = block.replace('{{', '{').replace('}}', '}')
        fake = self.home / 'bin/curl'
        fake.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
path = pathlib.Path(args[args.index('--config') + 1])
assert path.stat().st_mode & 0o777 == 0o600
raw = path.read_text().strip()
header = json.loads(raw.split(' = ', 1)[1])
expected = os.environ['CORTEX_TOKEN'] if args[-1].endswith('/mcp') else os.environ['CORTEX_API_TOKEN']
assert header == 'Authorization: Bearer ' + expected
assert not any(expected in value for value in args)
if '--data' in args:
    request = json.loads(args[args.index('--data') + 1])
    result = {'serverInfo': {}} if request['id'] == 1 else {'content': []}
    print(json.dumps({'id': request['id'], 'result': result}))
else: print('{}')
''')
        fake.chmod(0o700)
        token = 'secret$ dollar \"quote\" \\ slash # suffix'
        api_token = 'api \"quote\" \\ $ dollar # suffix'
        result = subprocess.run(['sh', '-s'], input=block, text=True, capture_output=True,
            env=dict(self.env, CORTEX_TOKEN=token, CORTEX_API_TOKEN=api_token, CORTEX_AUTH_MODE='bearer'), timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(token, result.stdout + result.stderr)
        self.assertNotIn(api_token, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count('__CORTEX_REPLY__'), 3)

    def test_compose_receives_quoted_managed_paths_without_shell_execution(self) -> None:
        # Docker, not this helper, owns dotenv expansion/decoding. Verify the
        # helper passes the file path intact even when target paths have spaces.
        previous = self.home
        moved = previous.with_name(previous.name + ' managed path')
        previous.rename(moved)
        self.addCleanup(lambda: moved.rename(previous) if moved.exists() else None)
        self.home = moved
        self.env['TEST_HOME'] = str(moved)
        self.env['PATH'] = str(moved / 'bin') + ':' + os.environ['PATH']
        (moved / '.env').write_text('CORTEX_TOKEN="quoted$credential"\nCORTEX_BACKUP_DIR="' + str(moved / 'backups') + '"\n')
        result = self.run_helper('create')
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        compose_calls = [call for call in calls if call[0] == 'compose']
        self.assertTrue(compose_calls)
        for call in compose_calls:
            self.assertEqual(call[call.index('--env-file') + 1], str(moved / '.env'))
        self.assertNotIn('quoted$credential', result.stdout + result.stderr)

    def test_schedule_is_idempotent_and_preserves_unrelated_jobs(self) -> None:
        original = '3 4 * * * /unrelated/task\n'
        (self.home / 'crontab').write_text(original)
        for _ in range(2):
            result = self.run_helper('schedule-install')
            self.assertEqual(result.returncode, 0, result.stderr)
        installed = (self.home / 'crontab').read_text()
        self.assertEqual(installed.count('setup backup create'), 1)
        self.assertTrue(installed.startswith(original))
        self.assertEqual(self.run_helper('schedule-check').returncode, 0)
        self.assertEqual(self.run_helper('schedule-remove').returncode, 0)
        self.assertEqual((self.home / 'crontab').read_text(), original)
        self.assertNotEqual(self.run_helper('schedule-check').returncode, 0)

if __name__ == '__main__': unittest.main()
