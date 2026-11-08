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
