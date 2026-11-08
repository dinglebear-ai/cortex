#!/usr/bin/env python3
"""Hermetic sweep regressions; HTTP below is a bounded loopback protocol fixture."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
import unittest
from unittest.mock import patch

BINARY = os.environ.get("CORTEX_SWEEP_CORTEX_BIN")
ROOT = Path(__file__).resolve().parents[4]
spec = importlib.util.spec_from_file_location("cli_sweep", Path(__file__).with_name("cli_sweep.py"))
sweep = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sweep)
backup_spec = importlib.util.spec_from_file_location("backup_sweep", Path(__file__).with_name("backup_sweep.py"))
backup = importlib.util.module_from_spec(backup_spec)
backup_spec.loader.exec_module(backup)


class SweepSetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        (self.home / ".cortex").mkdir(mode=0o700)
        (self.home / ".codex").mkdir(mode=0o700)
        self.env = {
            "LIVE_RUN_HOME": str(self.home), "LIVE_RUN_TMP": str(self.home),
            "LIVE_DOCKER_BIN": "/unused/docker", "LIVE_CANDIDATE_ID": "fixture",
            "LIVE_API_TOKEN": "fixture-rest-token", "LIVE_ADMIN_TOKEN": "fixture-admin-token",
            "LIVE_CORTEX_TOKEN": "fixture-mcp-token", "LIVE_CORTEX_URL": "http://127.0.0.1:61001",
            "LIVE_CLI_PATH": "/usr/bin:/bin", "LIVE_SETUP_CLI_PATH": "/usr/bin:/bin",
            "LIVE_CANDIDATE_VERSION": "fixture-version", "LIVE_DOCKER_COMPOSE_BIN": "/unused/compose",
            "CORTEX_COMPOSE_PROGRAM": "/unused/compose",
        }
        self.environment = patch.dict(os.environ, self.env, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def test_setup_verification_uses_candidate_port_and_credentials(self):
        with patch.object(sweep.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, b"{}", b"")) as run:
            self.assertEqual(sweep.run("/fixture/cortex", "setup repair", ["--json"], local_only=True)["exit"], 0)
        env = run.call_args.kwargs["env"]
        self.assertEqual(env["CORTEX_HOME"], str(self.home / ".cortex"))
        self.assertEqual(env["CORTEX_PORT"], "61001")
        self.assertEqual(env["CORTEX_SETUP_READY_TIMEOUT_SECS"], "5")
        self.assertLessEqual(int(env["CORTEX_RECEIVER_PORT"]), 65535)
        self.assertEqual(env["CORTEX_TOKEN"], self.env["LIVE_CORTEX_TOKEN"])
        self.assertEqual(env["CORTEX_API_TOKEN"], self.env["LIVE_API_TOKEN"])
        self.assertEqual(run.call_args.kwargs["timeout"], 20)

    def test_timeout_retains_bounded_output_and_names_failed_command(self):
        with patch.object(sweep.subprocess, "run", side_effect=subprocess.TimeoutExpired(
                ["cortex"], 20, output=b"phase started", stderr=b"diagnostic")):
            result = sweep.run("/fixture/cortex", "setup repair", ["--json"])
        self.assertEqual(result["exit"], 124)
        self.assertTrue(result["timeout"])
        self.assertIn("phase starteddiagnostic", result["terminal"])
        self.assertIn("setup repair exceeded the 20s", result["terminal"])

    def test_backup_arguments_match_normal_entrypoint(self):
        self.assertEqual(backup.backup_args("setup backup"), ["create"])
        self.assertEqual(backup.backup_args("setup backup schedule"), ["schedule", "install"])
        for operation in ("restore", "rollback"):
            self.assertEqual(backup.backup_args(f"setup backup {operation}", "saved-1"),
                             [operation, "saved-1", "--yes"])
            with self.assertRaisesRegex(ValueError, "verified recovery timestamp"):
                backup.backup_args(f"setup backup {operation}")
        for operation in ("install", "check", "remove"):
            self.assertEqual(backup.backup_args(f"setup backup schedule {operation}"),
                             ["schedule", operation])

    def test_backup_stamp_requires_real_verified_creation_output(self):
        self.assertEqual(backup.recovery_stamp(
            "verified recovery snapshot: /private/backups/recovery-saved-1 (restore with cortex)"), "saved-1")
        with self.assertRaisesRegex(ValueError, "verified recovery snapshot"):
            backup.recovery_stamp("backup started")

    def test_backup_marker_uses_owned_container_database(self):
        os.environ.update(LIVE_DOCKER_BIN="/fixture/docker", LIVE_RUN_ID="owned-run")
        replies = [subprocess.CompletedProcess([], 0, "owned-run\n", ""),
                   subprocess.CompletedProcess([], 0, "1\n", "")]
        with patch.object(backup.subprocess, "run", side_effect=replies) as observed:
            self.assertEqual(backup.marker(1), 1)
        command = observed.call_args.args[0]
        self.assertEqual(command[:7], ["/fixture/docker", "exec", "--user", "0:0", "cortex", "sqlite3", "/data/cortex.db"])
        self.assertIn("BEGIN IMMEDIATE", command[7])
        with patch.object(backup.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "foreign-run", "")) as observed:
            with self.assertRaisesRegex(RuntimeError, "ownership changed"):
                backup.marker()
            self.assertEqual(observed.call_count, 1)

    def test_backup_topology_renders_owned_managed_contract_without_starting_services(self):
        tools = self.home / "tools"
        tools.mkdir()
        docker = tools / "docker"
        docker.write_text('#!/bin/sh\ncase "$1" in info|ps) exit 0;; compose) exit 42;; *) exit 64;; esac\n')
        docker.chmod(0o700)
        os.environ.update(PATH=f"{tools}:/usr/bin:/bin", LIVE_PROJECT_ROOT=str(ROOT),
                          LIVE_RUN_ROOT=str(self.home), LIVE_COMPOSE_PROJECT="owned-live",
                          LIVE_RUN_ID="owned-run", LIVE_CLI_FIXTURE_BIN=str(ROOT / "tests/live/fixtures/surfaces/bin"),
                          LIVE_CURSOR_SIGNING_KEY="fixture-signing-key", LIVE_SERVER_INSTANCE_ID="fixture-server")
        script = ('set -eu; source "$1"; surface_cli_resource_register() { :; }; live_timeout_process_tree() { shift; "$@"; }; '
                  'surface_backup_topology_run /unused/cortex cortex:fixture')
        result = subprocess.run(["/bin/bash", "-c", script, "_", str(ROOT / "tests/live/phases/surfaces/backup-topology.sh")],
                                capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 42, result.stderr)
        home = self.home / "backup-home/.cortex"
        config = json.loads((home / "compose/docker-compose.yml").read_text())
        self.assertEqual(config["name"], "owned-live-surface-backup")
        service = config["services"]["cortex"]
        self.assertEqual(service["container_name"], "cortex")
        self.assertEqual(service["labels"], {"cortex.live.run_id": "owned-run"})
        self.assertEqual(service["volumes"], [f"{home}/data:/data", f"{home}/backups:/backups", f"{home}:/cortex-home"])
        self.assertRegex(service["ports"][0], r"^127[.]0[.]0[.]1:[1-9][0-9]*:3100/tcp$")
        self.assertEqual(service["environment"]["CORTEX_HOST"], "0.0.0.0")
        self.assertEqual(service["environment"]["CORTEX_PORT"], "3100")
        self.assertEqual(service["env_file"], [str(home / ".env")])
        self.assertNotIn("fixture-rest-token", json.dumps(config))

    def test_backup_crontab_fixture_preserves_install_check_remove_state(self):
        fixture = ROOT / "tests/live/fixtures/surfaces/bin/crontab"
        env = {"PATH": "/usr/bin:/bin", "TMPDIR": str(self.home)}
        absent = subprocess.run([str(fixture), "-l"], capture_output=True, timeout=5, env=env)
        self.assertEqual(absent.returncode, 1)
        self.assertIn(b"no crontab", absent.stderr)
        schedule = self.home / "schedule"
        schedule.write_text("0 */6 * * * /private/cortex setup backup create # cortex-backup-1\n")
        subprocess.run([str(fixture), str(schedule)], check=True, timeout=5, env=env)
        installed = subprocess.run([str(fixture), "-l"], check=True, capture_output=True, timeout=5, env=env)
        self.assertEqual(installed.stdout.decode(), schedule.read_text())
        schedule.write_text("")
        subprocess.run([str(fixture), str(schedule)], check=True, timeout=5, env=env)
        removed = subprocess.run([str(fixture), "-l"], check=True, capture_output=True, timeout=5, env=env)
        self.assertEqual(removed.stdout, b"")

    def test_setup_start_supplies_explicit_role(self):
        tail, _, execute = sweep.semantic_args({"spelling": "setup start", "mutation": "local"}, False)
        self.assertTrue(execute)
        self.assertIn("--role", tail)
        self.assertEqual(tail[tail.index("--role") + 1], "client")

    def test_ssh_fixture_reports_platform_from_command_argument(self):
        fixture = ROOT / "tests/live/fixtures/surfaces/bin/ssh"
        result = subprocess.run([str(fixture), "--", "cortex-live.invalid", "uname -s && uname -m"],
                                input=b"", capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"Linux\nx86_64\n")

    @unittest.skipUnless(BINARY, "set CORTEX_SWEEP_CORTEX_BIN for compiled deployment integration")
    def test_compiled_agent_deployment_uses_isolated_ssh_fixture(self):
        fixtures = ROOT / "tests/live/fixtures/surfaces/bin"
        os.environ["LIVE_SETUP_CLI_PATH"] = f"{fixtures}:/usr/bin:/bin"
        result = sweep.run(BINARY, "setup deploy agent", ["--hosts", "cortex-live.invalid", "--json"])
        self.assertEqual(result["exit"], 0, result)
        self.assertIn("deploying", result["terminal"])

    @unittest.skipUnless(BINARY, "set CORTEX_SWEEP_CORTEX_BIN for compiled watch integration")
    def test_smokewatch_observes_real_watcher_ingestion_and_checkpoint_pruning(self):
        (self.home / ".claude/projects").mkdir(parents=True)
        os.environ.update(LIVE_CURSOR_SIGNING_KEY="fixture-cursor-signing-key",
                          LIVE_SERVER_INSTANCE_ID="fixture-instance")
        result = sweep.run_long_lived(BINARY, "sessions smokewatch")
        self.assertEqual(result["exit"], 0, result)
        report = json.loads(result["terminal"])
        self.assertTrue(report["ingested"])
        self.assertTrue(report["pruned_missing_checkpoint"])
        self.assertTrue(result["terminated_for_cleanup"])

    def test_non_loopback_candidate_is_rejected(self):
        os.environ["LIVE_CORTEX_URL"] = "http://remote.invalid:3100"
        with self.assertRaisesRegex(ValueError, "run-owned loopback"):
            sweep.run("/fixture/cortex", "setup repair", ["--json"])

    @unittest.skipUnless(BINARY, "set CORTEX_SWEEP_CORTEX_BIN for compiled setup integration")
    def test_compiled_repair_verifies_authenticated_protocols(self):
        binary = BINARY
        calls = []
        reject_rest = False
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass
            def respond(self, status, payload):
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps(payload).encode())
            def do_GET(self):
                calls.append((self.path, self.headers.get("Authorization")))
                if len(calls) > 12:
                    return self.respond(429, {})
                if self.path == "/health":
                    return self.respond(200, {"status": "ok"})
                if self.path == "/api/stats" and not reject_rest and self.headers.get("Authorization") == "Bearer fixture-rest-token":
                    return self.respond(200, {"logs": 0})
                self.respond(401, {})
            def do_POST(self):
                calls.append((self.path, self.headers.get("Authorization")))
                if len(calls) > 12 or self.path != "/mcp" or self.headers.get("Authorization") != "Bearer fixture-mcp-token":
                    return self.respond(401, {})
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                result = {"serverInfo": {"name": "fixture"}} if request["method"] == "initialize" else {"content": []}
                self.respond(200, {"jsonrpc": "2.0", "id": request["id"], "result": result})
        server = HTTPServer(("127.0.0.1", 0), Handler)
        server.timeout = 1
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        self.addCleanup(server.server_close)
        self.addCleanup(worker.join, 2)
        self.addCleanup(server.shutdown)
        os.environ["LIVE_CORTEX_URL"] = f"http://127.0.0.1:{server.server_port}"
        fixtures = ROOT / "tests/live/fixtures/surfaces/bin"
        tools = self.home / "tools"
        tools.mkdir()
        for command, fixture in (("docker", "docker-setup"), ("systemctl", "systemctl"), ("ss", "ss"), ("timeout", "timeout")):
            (tools / command).symlink_to(fixtures / fixture)
        os.environ["LIVE_SETUP_CLI_PATH"] = f"{tools}:/usr/bin:/bin"
        result = sweep.run(binary, "setup repair", ["--json"], local_only=True)
        self.assertEqual(result["exit"], 0, result)
        report = json.loads(result["terminal"])
        self.assertEqual(Path(report["home"]), self.home / ".cortex")
        self.assertEqual(Path(report["env_path"]), self.home / ".cortex/.env")
        for relative in (".cortex/.env", ".cortex/compose/docker-compose.yml", ".cortex/compose/config/Dockerfile", ".cortex/deployments.toml"):
            self.assertTrue((self.home / relative).is_file(), relative)
        phases = report["phases"]
        for name in ("health", "mcp-auth", "rest-auth"):
            self.assertTrue(any(p["name"] == name and p["status"] == "ok" for p in phases), phases)
        self.assertEqual(calls, [("/health", None), ("/mcp", "Bearer fixture-mcp-token"),
                                 ("/mcp", "Bearer fixture-mcp-token"), ("/api/stats", "Bearer fixture-rest-token")])
        # Readiness alone must never qualify the authenticated setup gate.
        reject_rest = True
        result = sweep.run(binary, "setup repair", ["--json"], local_only=True)
        self.assertNotEqual(result["exit"], 0, result)
        phases = json.JSONDecoder().raw_decode(result["terminal"])[0]["phases"]
        self.assertTrue(any(p["name"] == "rest-auth" and p["status"] == "error" for p in phases), phases)


if __name__ == "__main__":
    unittest.main()
