#!/usr/bin/env python3
"""Exercise real Cortex onboarding against a disposable local server.

Build with cargo build --locked, then run this script. No Docker, installed
collector, home configuration, or external server is modified.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('CORTEX_TEST_BINARY', ROOT / '.cache/cargo/debug/cortex')).resolve()


def port() -> int:
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


class SetupRuntimeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        if not BINARY.is_file():
            raise RuntimeError('Build Cortex first with cargo build --locked')
        cls.tmp = tempfile.TemporaryDirectory(prefix='cortex-setup-runtime-')
        cls.root = Path(cls.tmp.name)
        cls.server_home = cls.root / 'server'
        cls.server_home.mkdir()
        cls.base_env = {k: v for k, v in os.environ.items()
                        if not k.startswith('CORTEX_') and k not in ('NO_AUTH', 'CODEX_HOME')}
        cls.http_port, cls.syslog_port = port(), port()
        cls.url = f'http://127.0.0.1:{cls.http_port}'
        env = dict(cls.base_env, HOME=str(cls.server_home), CORTEX_HOME=str(cls.server_home / '.cortex'),
                   CORTEX_DB_PATH=str(cls.server_home / 'cortex.db'), CORTEX_HOST='127.0.0.1',
                   CORTEX_PORT=str(cls.http_port), CORTEX_RECEIVER_HOST='127.0.0.1',
                   CORTEX_RECEIVER_PORT=str(cls.syslog_port), CORTEX_TOKEN='setup-runtime-mcp',
                   CORTEX_API_TOKEN='setup-runtime-rest', CORTEX_MIN_FREE_DISK_MB='0',
                   CORTEX_AUTH_MODE='bearer', CORTEX_ALLOWED_HOSTS='127.0.0.1,localhost,0.0.0.0', RUST_LOG='warn')
        cls.log = (cls.root / 'server.log').open('w+')
        cls.server = subprocess.Popen([str(BINARY), 'serve', 'mcp'], cwd=cls.server_home,
                                      env=env, stdout=cls.log, stderr=cls.log)
        deadline = time.monotonic() + 30
        health_error = None
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(cls.url + '/health', timeout=1) as response:
                    if response.status == 200:
                        return
            except (OSError, TimeoutError) as error:
                health_error = str(error)
            if cls.server.poll() is not None:
                break
            time.sleep(.15)
        cls.server.terminate()
        cls.server.wait(timeout=10)
        cls.log.seek(0)
        raise RuntimeError(f'Disposable server failed to start ({health_error}): ' + cls.log.read()[-3000:])

    @classmethod
    def tearDownClass(cls) -> None:
        cls.server.terminate()
        try:
            cls.server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            cls.server.kill()
            cls.server.wait(timeout=5)
        cls.log.close()
        cls.tmp.cleanup()

    def setUp(self) -> None:
        self.home = self.root / self.id().split('.')[-1]
        self.home.mkdir()
        self.env = dict(self.base_env, HOME=str(self.home), CODEX_HOME=str(self.home / '.codex'),
                        CORTEX_HOME=str(self.home / '.cortex'))
        self.mcp = self.secret('mcp', 'setup-runtime-mcp')
        self.rest = self.secret('rest', 'setup-runtime-rest')

    def secret(self, name: str, value: str) -> str:
        path = self.home / name
        path.write_text(value + '\n')
        path.chmod(0o600)
        return str(path)

    def run_cli(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(BINARY), *args], cwd=self.home, env=self.env,
                              text=True, capture_output=True, timeout=30)

    def test_real_authenticated_verification_and_wrong_credentials(self) -> None:
        args = ['setup', 'verify', '--server', self.url, '--token-file', self.mcp,
                '--api-token-file', self.rest, '--json']
        success = self.run_cli(*args)
        self.assertEqual(success.returncode, 0, success.stderr + success.stdout)
        phases = json.loads(success.stdout)
        self.assertTrue(all(p['status'] == 'ok' for p in phases), phases)
        wrong = self.secret('wrong', 'deliberately-wrong')
        failure = self.run_cli('setup', 'verify', '--server', self.url,
                               '--token-file', wrong, '--api-token-file', self.rest, '--json')
        self.assertNotEqual(failure.returncode, 0)
        self.assertNotIn('deliberately-wrong', failure.stdout + failure.stderr)

    def test_client_onboarding_all_clients_and_saved_repeat(self) -> None:
        (self.home / '.claude.json').write_text('{"theme":"preserved","mcpServers":{"other":{"command":"other"}}}\n')
        result = self.run_cli('setup', 'start', '--role', 'client', '--server', self.url,
                              '--token-file', self.mcp, '--api-token-file', self.rest,
                              '--clients', 'codex,claude,gemini', '--json')
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        report = json.loads(result.stdout)
        self.assertFalse(report['has_errors'])
        self.assertEqual(len(report['client_paths']), 3)
        self.assertFalse((self.home / '.cortex/data').exists())
        config = json.loads((self.home / '.claude.json').read_text())
        self.assertEqual(config['theme'], 'preserved')
        self.assertEqual(config['mcpServers']['other']['command'], 'other')
        files = [Path(p) for p in report['client_paths']]
        before = [p.read_bytes() for p in files]
        repeat = self.run_cli('setup', 'start', '--json')
        self.assertEqual(repeat.returncode, 0, repeat.stderr + repeat.stdout)
        self.assertEqual(before, [p.read_bytes() for p in files])
        self.assertNotIn('setup-runtime-mcp', repeat.stdout)
        self.assertNotIn('setup-runtime-rest', repeat.stdout)


    def test_rest_only_client_onboarding_and_saved_repeat(self) -> None:
        result = self.run_cli('setup', 'start', '--role', 'client', '--server', self.url,
                              '--api-token-file', self.rest, '--clients', 'none', '--json')
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        report = json.loads(result.stdout)
        self.assertEqual(report['client_paths'], [])
        self.assertFalse(any(p['name'] == 'mcp-auth' for p in report['phases']))
        self.assertTrue(any(p['name'] == 'rest-auth' and p['status'] == 'ok' for p in report['phases']))
        repeat = self.run_cli('setup', 'start', '--json')
        self.assertEqual(repeat.returncode, 0, repeat.stderr + repeat.stdout)
        self.assertFalse((self.home / '.cortex/data').exists())

    def test_malformed_client_config_never_echoes_credentials(self) -> None:
        codex = self.home / '.codex'
        codex.mkdir()
        (codex / 'config.toml').write_text(
            '[mcp_servers.cortex]\nhttp_headers = { Authorization = "Bearer DIAGNOSTIC_CANARY", BROKEN }\n')
        result = self.run_cli('setup', 'start', '--role', 'client', '--clients', 'codex',
                              '--dry-run', '--json')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('DIAGNOSTIC_CANARY', result.stdout + result.stderr)
        self.assertIn('line 2', result.stderr)

    def test_client_transition_requires_owned_docker_agent_removal(self) -> None:
        managed = self.home / '.cortex'
        compose = managed / 'heartbeat-agent-compose'
        compose.mkdir(parents=True)
        (compose / 'docker-compose.yml').write_text('services:\n  cortex-heartbeat-agent:\n    image: fixture\n')
        agent_env = managed / 'heartbeat-agent.env'
        agent_env.write_text('CORTEX_HEARTBEAT_TOKEN=setup-runtime-mcp\nCORTEX_AGENT_DOCKER=true\n')
        agent_env.chmod(0o600)
        fake_bin = self.home / 'fake-bin'
        fake_bin.mkdir()
        docker = fake_bin / 'docker'
        docker.write_text('#!/bin/sh\nprintf "%s\\n" "$CORTEX_HOME/heartbeat-agent-compose"\n')
        docker.chmod(0o700)
        self.env['PATH'] = str(fake_bin) + os.pathsep + self.env.get('PATH', '')
        args = ('setup', 'start', '--role', 'client', '--server', self.url,
                '--token-file', self.mcp, '--api-token-file', self.rest,
                '--clients', 'none', '--capabilities', 'none', '--json')
        blocked = self.run_cli(*args)
        self.assertNotEqual(blocked.returncode, 0)
        self.assertIn('heartbeatagent remove', blocked.stderr)
        self.assertFalse((managed / 'setup.toml').exists())
        self.assertIn('CORTEX_AGENT_DOCKER=true', agent_env.read_text())
        # Explicit removal leaves credentials/config for inspection; no owned
        # container remains. The role transition can then proceed.
        docker.write_text('#!/bin/sh\nexit 0\n')
        result = self.run_cli(*args)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn('role = "client"', (managed / 'setup.toml').read_text())

    def test_overlay_consent_spellings_reach_authenticated_checks(self) -> None:
        # 0.0.0.0 reaches this local fixture while the credential transport
        # policy correctly treats it as non-loopback. Disable ambient proxies.
        url = self.url.replace('127.0.0.1', '0.0.0.0')
        for key in ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy']:
            self.env.pop(key, None)
        self.env['NO_PROXY'] = '*'
        for consent in ['true', '1', 'TRUE']:
            self.env['CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP'] = consent
            result = self.run_cli('setup', 'verify', '--server', url, '--token-file', self.mcp,
                                  '--api-token-file', self.rest, '--json')
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            self.assertTrue(all(p['status'] == 'ok' for p in json.loads(result.stdout)))
        self.env['CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP'] = 'false'
        failure = self.run_cli('setup', 'verify', '--server', url, '--token-file', self.mcp,
                               '--api-token-file', self.rest, '--json')
        self.assertNotEqual(failure.returncode, 0)
        self.assertTrue(any(p['name'] == 'credential-transport' and p['status'] == 'error'
                            for p in json.loads(failure.stdout)))

    def test_existing_agent_enrollment_preview_reuses_saved_token(self) -> None:
        managed = self.home / '.cortex'
        managed.mkdir()
        env_file = managed / 'heartbeat-agent.env'
        env_file.write_text(f'CORTEX_HEARTBEAT_TOKEN=setup-runtime-mcp\nCORTEX_HEARTBEAT_TARGET={self.url}\n')
        env_file.chmod(0o600)
        result = self.run_cli('setup', 'start', '--role', 'agent', '--dry-run', '--json')
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(json.loads(result.stdout)['server'], self.url)
        self.assertNotIn('setup-runtime-mcp', result.stdout)
        self.assertFalse((managed / 'setup.toml').exists())

    def test_canonical_no_auth_onboards_client_without_a_token(self) -> None:
        server_home = self.home / 'unauth-server'
        server_home.mkdir()
        http_port, syslog_port = port(), port()
        url = f'http://127.0.0.1:{http_port}'
        env = dict(self.base_env, HOME=str(server_home), CORTEX_HOME=str(server_home / '.cortex'),
                   CORTEX_DB_PATH=str(server_home / 'cortex.db'), CORTEX_HOST='127.0.0.1',
                   CORTEX_PORT=str(http_port), CORTEX_RECEIVER_HOST='127.0.0.1',
                   CORTEX_RECEIVER_PORT=str(syslog_port), CORTEX_MIN_FREE_DISK_MB='0',
                   CORTEX_NO_AUTH='true', NO_AUTH='false', CORTEX_API_TOKEN='setup-runtime-rest', RUST_LOG='warn')
        with (server_home / 'server.log').open('w+') as log:
            server = subprocess.Popen([str(BINARY), 'serve', 'mcp'], cwd=server_home,
                                      env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    try:
                        with urllib.request.urlopen(url + '/health', timeout=1):
                            break
                    except (OSError, TimeoutError):
                        if server.poll() is not None:
                            log.seek(0)
                            self.fail(log.read()[-2000:])
                        time.sleep(.15)
                result = self.run_cli('setup', 'start', '--role', 'client', '--server', url,
                                      '--clients', 'codex', '--api-token-file', self.rest, '--set', 'CORTEX_NO_AUTH=true',
                                      '--set', 'NO_AUTH=false', '--json')
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
                self.assertFalse(json.loads(result.stdout)['has_errors'])
                self.assertNotIn('Authorization', (self.home / '.codex/config.toml').read_text())
            finally:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
