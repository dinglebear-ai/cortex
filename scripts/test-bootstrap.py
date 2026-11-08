#!/usr/bin/env python3
"""Offline bootstrap fixtures: no installed binaries, network, or running services."""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT: Path = Path(__file__).resolve().parents[1]


class BootstrapTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.bin = self.root / 'tools'
        self.bin.mkdir()
        self.log = self.root / 'setup-args'
        self.env = dict(os.environ, HOME=str(self.root), SHELL='/bin/zsh', PATH=f'{self.bin}:{os.environ["PATH"]}',
                        CORTEX_INSTALL_PREFIX=str(self.root / 'prefix'),
                        CORTEX_INSTALL_SKIP_SETUP='0')
        for key in list(self.env):
            if key.startswith('CORTEX_INSTALL_') and key not in ('CORTEX_INSTALL_PREFIX', 'CORTEX_INSTALL_SKIP_SETUP'):
                del self.env[key]
        self.env.pop('CORTEX_VERSION', None)
        self.write('uname', '#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
        self.binary = self.root / 'cortex'
        self.binary.write_text(f'#!/bin/sh\nprintf "%s\\n" "$@" > "{self.log}"\n')
        self.binary.chmod(0o755)
        self.archive = self.root / 'release.tar.gz'
        with tarfile.open(self.archive, 'w:gz') as archive:
            archive.add(self.binary, arcname='cortex')
        self.digest = self.root / 'checksum'
        self.digest.write_text(hashlib.sha256(self.archive.read_bytes()).hexdigest() + '  cortex-linux-x86_64.tar.gz\n')
        self.write('curl', f'''#!/bin/sh
printf '%s\\n' "$2" >> '{self.root}/urls'
case "$2" in *.sha256) cp '{self.digest}' "$4";; *) cp '{self.archive}' "$4";; esac
''')

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, name: str, contents: str) -> None:
        path = self.bin / name
        path.write_text(contents)
        path.chmod(0o755)

    def run_install(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(['sh', str(ROOT / 'install.sh'), *args], env=self.env,
                              capture_output=True, text=True)

    def test_supported_platforms_and_asset_names(self) -> None:
        for system, arch, asset in [('Linux', 'x86_64', 'cortex-linux-x86_64.tar.gz'),
                                     ('Linux', 'aarch64', 'cortex-linux-aarch64.tar.gz'),
                                     ('Darwin', 'arm64', 'cortex-macos-arm64')]:
            with self.subTest(system=system, arch=arch):
                self.write('uname', f'#!/bin/sh\ncase "$1" in -s) echo {system};; -m) echo {arch};; esac\n')
                source = self.binary if system == 'Darwin' else self.archive
                self.digest.write_text(hashlib.sha256(source.read_bytes()).hexdigest() + f'  {asset}\n')
                self.write('curl', f'#!/bin/sh\nprintf "%s\\n" "$2" >> "{self.root}/urls"\ncase "$2" in *.sha256) cp "{self.digest}" "$4";; *) cp "{source}" "$4";; esac\n')
                result = self.run_install('--role', 'agent')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(asset, (self.root / 'urls').read_text())
                self.assertEqual(self.log.read_text().splitlines(), ['setup', 'start', '--role', 'agent'])

    def test_checksum_failure_preserves_existing_binary(self) -> None:
        installed = self.root / 'prefix/bin/cortex'
        installed.parent.mkdir(parents=True)
        installed.write_text('previous binary')
        self.digest.write_text('0' * 64)
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('checksum mismatch', result.stderr)
        self.assertEqual(installed.read_text(), 'previous binary')

    def test_download_failure_does_not_install(self) -> None:
        self.write('curl', '#!/bin/sh\nexit 22\n')
        self.assertNotEqual(self.run_install().returncode, 0)
        self.assertFalse((self.root / 'prefix/bin/cortex').exists())

    def test_skip_setup(self) -> None:
        self.env['CORTEX_INSTALL_SKIP_SETUP'] = '1'
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.log.exists())

    def test_source_build_uses_cargo_target_directory_and_lock(self) -> None:
        target = self.root / 'custom-target'
        (target / 'release').mkdir(parents=True)
        (target / 'release/cortex').write_bytes(self.binary.read_bytes())
        self.write('cargo', f'''#!/bin/sh
printf '%s\\n' "$*" >> '{self.root}/cargo-args'
case "$1" in metadata) printf '{{"target_directory":"{target}"}}';; build) exit 0;; esac
''')
        self.env['CORTEX_INSTALL_METHOD'] = 'build'
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('build --release --locked --bin cortex', (self.root / 'cargo-args').read_text())

    def test_failed_source_build_does_not_install(self) -> None:
        self.write('cargo', '#!/bin/sh\ncase "$1" in metadata) echo \'{"target_directory":"/missing"}\';; build) exit 42;; esac\n')
        self.env['CORTEX_INSTALL_METHOD'] = 'build'
        self.assertNotEqual(self.run_install().returncode, 0)
        self.assertFalse((self.root / 'prefix/bin/cortex').exists())

    def test_path_configuration_is_durable_and_idempotent(self) -> None:
        self.env['CORTEX_INSTALL_PREFIX'] = str(self.root / "prefix with ' quote")
        for _ in range(2):
            result = self.run_install()
            self.assertEqual(result.returncode, 0, result.stderr)
        profile = self.root / '.zshrc'
        self.assertEqual(profile.read_text().count('# Cortex CLI'), 1)
        result = subprocess.run(['sh', '-c', '. "$1"; command -v cortex', 'sh', str(profile)],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), self.env['CORTEX_INSTALL_PREFIX'] + '/bin/cortex')

    def test_compatibility_adapter_uses_canonical_installer(self) -> None:
        self.env.pop('CORTEX_INSTALL_SKIP_SETUP')
        self.env['INSTALL_DIR'] = str(self.root / 'compat-bin')
        self.env['CORTEX_RMCP_VERSION'] = '9.9.9'
        result = subprocess.run(['bash', str(ROOT / 'scripts/install.sh')], env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / 'compat-bin/cortex').exists())
        self.assertIn('/v9.9.9/cortex-linux-x86_64.tar.gz', (self.root / 'urls').read_text())
        self.assertFalse(self.log.exists())

    def test_piped_compatibility_adapter_fetches_bootstrap_and_forwards_args(self) -> None:
        self.env['CORTEX_RMCP_VERSION'] = '9.9.9'
        self.env['INSTALL_DIR'] = str(self.root / 'compat-bin')
        self.write('curl', f'''#!/bin/sh
printf '%s\\n' "$2" >> '{self.root}/urls'
case "$2" in
  https://raw.githubusercontent.com/*/install.sh) cp '{ROOT / 'install.sh'}' "$4";;
  *.sha256) cp '{self.digest}' "$4";;
  *) cp '{self.archive}' "$4";;
esac
''')
        args = ['--role', 'client', '--server', 'https://cortex.example', '--clients', 'none', '--json']
        result = subprocess.run(['bash', '-s', '--', *args],
                                input=(ROOT / 'scripts/install.sh').read_text(), cwd=self.root,
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn('unbound variable', result.stderr)
        self.assertTrue((self.root / 'compat-bin/cortex').exists())
        self.assertEqual(self.log.read_text().splitlines(), ['setup', 'start', *args])
        urls = (self.root / 'urls').read_text()
        self.assertIn('/dinglebear-ai/cortex/v9.9.9/install.sh', urls)
        self.assertIn('/v9.9.9/cortex-linux-x86_64.tar.gz', urls)

    def test_setup_failure_propagates(self) -> None:
        self.binary.write_text('#!/bin/sh\nexit 17\n')
        with tarfile.open(self.archive, 'w:gz') as archive:
            archive.add(self.binary, arcname='cortex')
        self.digest.write_text(hashlib.sha256(self.archive.read_bytes()).hexdigest())
        self.assertEqual(self.run_install().returncode, 17)


if __name__ == '__main__':
    unittest.main()
