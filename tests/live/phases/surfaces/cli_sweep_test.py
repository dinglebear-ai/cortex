#!/usr/bin/env python3
"""Hermetic sweep regressions; HTTP below is a bounded loopback protocol fixture."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
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

    def test_backup_timeout_stops_separate_process_group_before_return(self):
        cli = self.home / "synthetic-cortex"
        cli.write_text("""#!/usr/bin/python3
import json, os, pathlib, signal, subprocess, sys, time
root = pathlib.Path(os.environ['HOME'])
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'], preexec_fn=os.setpgrp)
def finish(_signal, _frame):
    child.wait(timeout=3)
    raise SystemExit(0)
signal.signal(signal.SIGTERM, finish)
identities = [{'pid': pid, 'sid': os.getsid(pid), 'group': os.getpgid(pid),
               'started': subprocess.check_output(['ps', '-p', str(pid), '-o', 'lstart='], text=True).strip()}
              for pid in (os.getpid(), child.pid)]
(root / 'owned-pids').write_text(json.dumps(identities))
print('before timeout', flush=True)
time.sleep(60)
""")
        cli.chmod(0o700)
        original_popen = subprocess.Popen
        created = []
        def ready_process(argv, **kwargs):
            process = original_popen(argv, **kwargs)
            if argv[0] != str(cli):
                return process
            created.append(process)
            deadline = time.monotonic() + 5
            ready = False
            while time.monotonic() < deadline:
                try:
                    ready = len(json.loads((self.home / "owned-pids").read_text())) == 2
                except (OSError, ValueError):
                    pass
                if ready:
                    break
                if process.poll() is not None:
                    self.fail("synthetic CLI exited before its readiness receipt")
                time.sleep(0.01)
            self.assertTrue(ready, "synthetic child readiness exceeded 5s")
            return process
        self.addCleanup(self.stop_synthetic_processes, created)
        with patch.object(backup.subprocess, "Popen", side_effect=ready_process), \
                patch.object(backup, "bounded_timeout", return_value=0.2):
            with self.assertRaisesRegex(RuntimeError, "timed out.*recovery state retained"):
                backup.run(str(cli), ["create"], {"HOME": str(self.home), "PATH": "/usr/bin:/bin"})
        identities = json.loads((self.home / "owned-pids").read_text())
        self.assertEqual(identities[0]["sid"], identities[1]["sid"])
        self.assertNotEqual(identities[0]["group"], identities[1]["group"])
        for identity in identities:
            with self.assertRaises(ProcessLookupError):
                os.kill(identity["pid"], 0)
        self.assertIsNotNone(created[0].returncode)

    def stop_synthetic_processes(self, created):
        receipt = self.home / "owned-pids"
        if receipt.exists():
            for identity in json.loads(receipt.read_text()):
                pid = identity["pid"]
                try:
                    if os.getsid(pid) != identity["sid"]:
                        continue
                    started = subprocess.run(["ps", "-p", str(pid), "-o", "lstart="],
                                             capture_output=True, timeout=2, text=True).stdout.strip()
                    if started == identity["started"]:
                        os.kill(pid, 9)
                except ProcessLookupError:
                    pass
        for process in created:
            if process.poll() is None:
                # A missing receipt still leaves this new Popen session owned.
                backup.stop_session(process)
            process.wait(timeout=5)
            process.stdout.close()
            process.stderr.close()

    def test_backup_subprocess_preserves_normal_output(self):
        cli = self.home / "synthetic-cortex"
        cli.write_text("#!/bin/sh\nprintf 'verified recovery snapshot: /private/recovery-saved-1 (restore)\\n'\n")
        cli.chmod(0o700)
        observation = backup.run(str(cli), ["create"], {"PATH": "/usr/bin:/bin"})
        self.assertEqual(observation["exit"], 0)
        self.assertEqual(backup.recovery_stamp(observation["terminal"]), "saved-1")

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

    def test_nested_auth_fixture_uses_exact_disposable_paths_and_permissions(self):
        with patch.object(backup, "container_command", return_value="") as command:
            backup.prepare_auth_fixture()
        script = command.call_args.args[0][-1]
        self.assertIn("/cortex-home/recovery-auth-check/nested/auth.key", script)
        self.assertIn("chown 1000:1000", script)
        self.assertIn("chmod 0750", script)
        self.assertIn("chmod 0600", script)
        self.assertNotIn("chmod 0750 /cortex-home\n", script)
        with patch.object(backup, "container_command", return_value="") as command:
            backup.remove_auth_fixture()
        self.assertEqual(command.call_args.args[0], ["rm", "-rf", "--", "/cortex-home/recovery-auth-check"])

    def test_nested_auth_fixture_requires_exact_metadata_and_user_read_write(self):
        expected = "1000:1000:750\n1000:1000:750\n1000:1000:600\ncortex-live-nested-auth-fixture\n"
        with patch.object(backup, "container_command", side_effect=[expected, ""]) as command:
            backup.verify_auth_fixture()
        self.assertEqual(command.call_count, 2)
        user_call = command.call_args
        self.assertEqual(user_call.kwargs, {})
        script = user_call.args[0][-1]
        self.assertIn("cd '/cortex-home/recovery-auth-check'", script)
        self.assertIn("/bin/su -s /bin/sh cortex -c", script)
        self.assertIn('test "$(id -u)" = 1000', script)
        self.assertIn('test "$(id -g)" = 1000', script)
        self.assertLess(script.index("cd '/cortex-home/recovery-auth-check'"), script.index("/bin/su"))
        self.assertIn("cd -P nested", script)
        self.assertIn("cat auth.key", script)
        self.assertIn("> auth.key", script)
        for observed in [expected.replace("1000:1000:750", "0:0:750", 1),
                         expected.replace("1000:1000:750", "1000:1000:755", 1),
                         expected.replace("1000:1000:600", "0:0:600"),
                         expected.replace("nested-auth-fixture", "wrong-content")]:
            with self.subTest(observed=observed):
                with patch.object(backup, "container_command", return_value=observed) as command:
                    with self.assertRaisesRegex(RuntimeError, "nested auth fixture"):
                        backup.verify_auth_fixture()
                self.assertEqual(command.call_count, 1)
        with patch.object(backup, "container_command", side_effect=[expected, subprocess.CalledProcessError(1, ["docker", "exec"])]):
            with self.assertRaises(subprocess.CalledProcessError):
                backup.verify_auth_fixture()

    def test_nested_auth_fixture_cleanup_follows_completed_observations_and_preserves_exceptions(self):
        contract = self.home / "contract.json"
        contract.write_text(json.dumps({"entries": [
            {"kind": "cli", "spelling": f"setup backup {action}", "id": action, "aliases": []}
            for action in ("restore", "rollback")]}))
        output = self.home / "backup-results.json"
        os.environ.update(LIVE_BACKUP_HOME=str(self.home / ".cortex"), LIVE_BACKUP_PORT="32123",
                          LIVE_BACKUP_CLI_PATH="/usr/bin:/bin")
        for scenario in ("success", "aggregate-failure", "exception"):
            with self.subTest(scenario=scenario):
                output.unlink(missing_ok=True)
                events = []
                fixture = {"exists": False}
                def seed():
                    fixture["exists"] = True
                    events.append("seed")
                def remove():
                    if output.exists():
                        self.assertEqual(len(json.loads(output.read_text())["results"]), 6)
                        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
                        events.append("completed-cleanup")
                    else:
                        events.append("remove")
                    fixture["exists"] = False
                def verify():
                    events.append("verify")
                    if scenario == "exception" and ("cli", ("restore", "saved-1", "--yes")) in events:
                        raise RuntimeError("restored auth fixture verification failed")
                def execute(_binary, tail, _env):
                    events.append(("cli", tuple(tail)))
                    if "--help" in tail:
                        return {"exit": 0, "mentions_usage": True, "terminal": "Usage: backup"}
                    if "--cortex-live-invalid-option" in tail:
                        return {"exit": 2, "terminal": "unknown backup argument"}
                    if tail[0] in ("restore", "rollback"):
                        fixture["exists"] = True
                    terminal = ("verified recovery snapshot: /backups/recovery-saved-1 (restore)"
                                if tail == ["create"] else "authenticated server verification passed")
                    if scenario == "aggregate-failure" and tail[0] == "rollback":
                        terminal = "authenticated verification missing"
                    return {"exit": 0, "terminal": terminal}
                with patch.object(backup.sys, "argv", ["backup_sweep.py", str(contract), "/fixture/cortex", str(output)]), \
                        patch.object(backup, "_DEADLINE", None), \
                        patch.object(backup, "run", side_effect=execute), \
                        patch.object(backup, "marker", return_value=1), \
                        patch.object(backup, "verify_bundle"), \
                        patch.object(backup, "prepare_auth_fixture", side_effect=seed), \
                        patch.object(backup, "remove_auth_fixture", side_effect=remove), \
                        patch.object(backup, "verify_auth_fixture", side_effect=verify):
                    if scenario == "exception":
                        with self.assertRaisesRegex(RuntimeError, "restored auth fixture verification failed"):
                            backup.main()
                    else:
                        self.assertEqual(backup.main(), 0)
                self.assertEqual(events[:3], ["seed", "verify", ("cli", ("create",))])
                if scenario == "exception":
                    self.assertTrue(fixture["exists"])
                    self.assertNotIn("completed-cleanup", events)
                    self.assertEqual(events.count("remove"), 1)
                    self.assertFalse(output.exists())
                    continue
                self.assertFalse(fixture["exists"])
                self.assertEqual(events[-1], "completed-cleanup")
                for action in ("restore", "rollback"):
                    index = events.index(("cli", (action, "saved-1", "--yes")))
                    self.assertEqual(events[index - 1], "remove")
                    self.assertEqual(events[index + 1], "verify")
                payload = json.loads(output.read_text())
                positives = [record for record in payload["results"] if record["case_kind"] == "semantic-positive"]
                self.assertEqual(len(positives), 2)
                self.assertTrue(all(record["observation"]["restored_nested_auth"] for record in positives))
                self.assertEqual(bool(payload["failures"]), scenario == "aggregate-failure")

    def test_owned_container_failure_retains_bounded_redacted_stderr_without_arguments(self):
        os.environ.update(LIVE_DOCKER_BIN="/fixture/docker", LIVE_RUN_ID="owned-run")
        stderr = "Permission denied errno=13 fixture-rest-token " + "x" * 2048
        errors = [(subprocess.CalledProcessError(13, ["docker", "exec", "private-argument"], stderr=stderr), "exit 13"),
                  (subprocess.TimeoutExpired(["docker", "exec", "private-argument"], 10, stderr=stderr.encode()), "timeout after 10s")]
        for error, status in errors:
            with self.subTest(status=status):
                replies = [subprocess.CompletedProcess([], 0, "owned-run\n", ""), error]
                with patch.object(backup.subprocess, "run", side_effect=replies):
                    with self.assertRaises(RuntimeError) as raised:
                        backup.container_command(["sh", "-c", "private-argument"])
                diagnostic = str(raised.exception)
                self.assertIn("Permission denied errno=13", diagnostic)
                self.assertIn(status, diagnostic)
                self.assertNotIn("private-argument", diagnostic)
                self.assertNotIn("fixture-rest-token", diagnostic)
                self.assertLessEqual(len(diagnostic.split("stderr=", 1)[1]), 1024)
                self.assertTrue(raised.exception.__suppress_context__)

    def test_owned_container_auth_execution_retains_ten_second_limits(self):
        os.environ.update(LIVE_DOCKER_BIN="/fixture/docker", LIVE_RUN_ID="owned-run")
        replies = [subprocess.CompletedProcess([], 0, "owned-run\n", ""),
                   subprocess.CompletedProcess([], 0, "", "")]
        with patch.object(backup.subprocess, "run", side_effect=replies) as command:
            backup.container_command(["sh", "-c", "cd /cortex-home/recovery-auth-check; /bin/su -s /bin/sh cortex -c id"])
        self.assertEqual(command.call_args.args[0], ["/fixture/docker", "exec", "--user", "0:0", "cortex", "sh", "-c",
                                                   "cd /cortex-home/recovery-auth-check; /bin/su -s /bin/sh cortex -c id"])
        for call in command.call_args_list:
            self.assertGreater(call.kwargs["timeout"], 0)
            self.assertLessEqual(call.kwargs["timeout"], 10)
            self.assertTrue(call.kwargs["check"])

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

    def test_backup_topology_failure_stops_target_and_retains_recovery_files(self):
        tools = self.home / "tools"
        tools.mkdir()
        project_root = self.home / "fixture-project"
        runner = project_root / "tests/live/phases/surfaces/backup_sweep.py"
        runner.parent.mkdir(parents=True)
        runner.write_text("import os, pathlib\nhome=pathlib.Path(os.environ['LIVE_BACKUP_HOME'])\n(home/'backups'/'preserve').write_text('recovery evidence')\nraise SystemExit(124)\n")
        docker = tools / "docker"
        docker.write_text("""#!/bin/sh
case "$1:$*" in
  info:*|ps:*) exit 0 ;;
  inspect:*project.config_files*) printf '%s\n' "$LIVE_RUN_ROOT/backup-home/.cortex/compose/docker-compose.yml" ;;
  inspect:*compose.project*) printf '%s\n' owned-live-surface-backup ;;
  inspect:*cortex.live.run_id*) printf '%s\n' owned-run ;;
  compose:*'ps -q cortex'*) printf '%s\n' owned-container ;;
  compose:*'port cortex 3100'*) printf '%s\n' 127.0.0.1:32123 ;;
  compose:*'stop -t 10 cortex'*) printf '%s\n' stop >> "$LIVE_RUN_ROOT/operations" ;;
  compose:*down*) printf '%s\n' down >> "$LIVE_RUN_ROOT/operations" ;;
  compose:*) exit 0 ;;
  exec:*) printf '%s\n' DELETE >> "$LIVE_RUN_ROOT/operations" ;;
  *) exit 64 ;;
esac
""")
        docker.chmod(0o700)
        os.environ.update(PATH=f"{tools}:/usr/bin:/bin", LIVE_PROJECT_ROOT=str(project_root),
                          LIVE_RUN_ROOT=str(self.home), LIVE_COMPOSE_PROJECT="owned-live",
                          LIVE_RUN_ID="owned-run", LIVE_SURFACE_CONTRACT="/unused/contract",
                          LIVE_CURSOR_SIGNING_KEY="fixture-key", LIVE_SERVER_INSTANCE_ID="fixture-server",
                          LIVE_CLI_FIXTURE_BIN=str(ROOT / "tests/live/fixtures/surfaces/bin"))
        script = ('set -eu; source "$1"; surface_cli_resource_register() { :; }; '
                  'surface_cli_resource_created() { :; }; '
                  'surface_cli_resource_verified_removed() { echo verified >> "$LIVE_RUN_ROOT/operations"; }; '
                  'live_timeout_process_tree() { shift; "$@"; }; '
                  'surface_backup_topology_run /unused/cortex cortex:fixture')
        result = subprocess.run(["/bin/bash", "-c", script, "_", str(ROOT / "tests/live/phases/surfaces/backup-topology.sh")],
                                capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 124, result.stderr)
        home = self.home / "backup-home/.cortex"
        self.assertEqual((home / "backups/preserve").read_text(), "recovery evidence")
        self.assertIn("fixture-rest-token", (home / ".env").read_text())
        self.assertEqual((self.home / "operations").read_text().splitlines(), ["stop", "down", "verified"])
        self.assertIn(b"retained private recovery state", result.stderr)

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
