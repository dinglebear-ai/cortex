#!/usr/bin/env python3
"""Qualify recovery CLI against the run-owned managed Compose deployment."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

_DEADLINE = None


def bounded_timeout(limit):
    remaining = limit if _DEADLINE is None else min(limit, _DEADLINE - time.monotonic())
    if remaining <= 0:
        raise RuntimeError("managed backup sweep exhausted its 300s total budget")
    return remaining


def backup_args(spelling, stamp=None):
    action = spelling.removeprefix("setup backup").strip()
    if action in ("", "create"):
        return ["create"]
    if action in ("restore", "rollback"):
        if not stamp:
            raise ValueError("a verified recovery timestamp is required")
        return [action, stamp, "--yes"]
    if action in ("schedule", "schedule install"):
        return ["schedule", "install"]
    if action in ("schedule check", "schedule remove"):
        return action.split()
    raise ValueError(f"unsupported backup surface {spelling}")


def recovery_stamp(output):
    match = re.search(r"verified recovery snapshot: [^\n]*?/recovery-([A-Za-z0-9_.-]+) ", output)
    if not match:
        raise ValueError("create did not report a verified recovery snapshot")
    return match.group(1)


def container_command(arguments):
    docker = os.environ["LIVE_DOCKER_BIN"]
    identity = subprocess.run([docker, "inspect", "-f",
                              '{{ index .Config.Labels "cortex.live.run_id" }}', "cortex"],
                             capture_output=True, check=True, timeout=bounded_timeout(10), text=True)
    if identity.stdout.strip() != os.environ["LIVE_RUN_ID"]:
        raise RuntimeError("managed backup container ownership changed")
    return subprocess.run([docker, "exec", "--user", "0:0", "cortex", *arguments],
                          capture_output=True, check=True, timeout=bounded_timeout(10), text=True).stdout


def marker(value=None):
    sql = ""
    if value is not None:
        sql = ("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS cortex_live_backup_marker (value INTEGER NOT NULL); "
               "DELETE FROM cortex_live_backup_marker; "
               f"INSERT INTO cortex_live_backup_marker VALUES ({int(value)}); COMMIT; ")
    sql += "SELECT value FROM cortex_live_backup_marker;"
    return int(container_command(["sqlite3", "/data/cortex.db", sql]).strip())


def verify_bundle(stamp, protect=False):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", stamp):
        raise ValueError("unsafe recovery timestamp")
    bundle = f"/backups/recovery-{stamp}"
    observed = container_command(["cat", f"{bundle}/VERIFIED"]).strip()
    if observed != stamp:
        raise RuntimeError("create did not persist a matching VERIFIED bundle")
    if protect:
        container_command(["touch", f"{bundle}/PROTECTED"])


def session_members(session_id):
    # A lifecycle shell creates its own process group while retaining this SID.
    # Exclude zombies, which cannot execute or mutate recovery state.
    observed = subprocess.run(["ps", "-e", "-o", "pid=,stat="], capture_output=True,
                              check=True, timeout=2, text=True, env={"PATH": "/usr/bin:/bin"})
    if len(observed.stdout) > 1048576:
        raise RuntimeError("owned-session process inventory exceeded its output budget")
    members = []
    for line in observed.stdout.splitlines():
        pid_text, state = line.split(maxsplit=1)
        if state.startswith("Z"):
            continue
        pid = int(pid_text)
        try:
            if os.getsid(pid) == session_id:
                members.append(pid)
        except ProcessLookupError:
            pass
    return members


def stop_session(process):
    # Signal only the newly-created CLI session, across all of its groups.
    # Check SID again immediately before each signal to reject stale PID data.
    try:
        for signum in (signal.SIGTERM, signal.SIGKILL):
            deadline = time.monotonic() + 2
            while True:
                members = session_members(process.pid)
                if not members:
                    return
                for pid in sorted(members, key=lambda pid: pid == process.pid):
                    try:
                        if os.getsid(pid) == process.pid:
                            os.kill(pid, signum)
                    except ProcessLookupError:
                        pass
                if time.monotonic() >= deadline:
                    break
                time.sleep(0.05)
        if session_members(process.pid):
            raise RuntimeError("owned backup CLI session cancellation is ambiguous; recovery state retained")
    finally:
        # Only this direct child belongs to Popen and can be reaped here.
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)


def run(binary, tail, env):
    timeout = bounded_timeout(120)
    process = subprocess.Popen([binary, "setup", "backup", *tail], env=env,
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        if session_members(process.pid):
            raise RuntimeError("backup CLI exited with live session descendants; recovery state retained")
    except BaseException as error:
        try:
            stop_session(process)
        except Exception as cancellation_error:
            raise RuntimeError("owned backup CLI cancellation is ambiguous; recovery state retained") from cancellation_error
        if isinstance(error, subprocess.TimeoutExpired):
            output = ((error.stdout or b"") + (error.stderr or b""))[:65536]
            detail = output.decode("utf-8", "replace")[-1024:]
            raise RuntimeError(f"backup CLI timed out; owned session stopped; recovery state retained; {detail}") from error
        raise
    finally:
        process.stdout.close()
        process.stderr.close()
    output = (stdout + stderr)[:65536]
    return {"exit": process.returncode, "bytes": len(output),
            "sha256": hashlib.sha256(output).hexdigest(),
            "mentions_usage": b"Usage" in output,
            "terminal": output.decode("utf-8", "replace")[-8192:]}


def main():
    global _DEADLINE
    _DEADLINE = time.monotonic() + 300
    contract, binary, destination = sys.argv[1:]
    entries = [entry for entry in json.loads(Path(contract).read_text())["entries"]
               if entry["kind"] == "cli" and entry["spelling"].startswith("setup backup")]
    home = Path(os.environ["LIVE_BACKUP_HOME"])
    run_tmp = Path(os.environ["LIVE_RUN_TMP"])
    env = {key: value for key, value in os.environ.items()
           if key in ("DOCKER_HOST", "DOCKER_API_VERSION", "DOCKER_TLS_VERIFY", "DOCKER_CERT_PATH")}
    env.update({"PATH": os.environ["LIVE_BACKUP_CLI_PATH"], "HOME": str(home.parent),
                "CORTEX_HOME": str(home), "CODEX_HOME": str(home.parent / ".codex"),
                "XDG_CONFIG_HOME": str(home.parent / ".config"),
                "XDG_DATA_HOME": str(home.parent / ".local/share"),
                "XDG_CACHE_HOME": str(home.parent / ".cache"), "TMPDIR": str(run_tmp),
                "CORTEX_USE_HTTP": "false", "CORTEX_PORT": os.environ["LIVE_BACKUP_PORT"],
                "CORTEX_API_TOKEN": os.environ["LIVE_API_TOKEN"],
                "CORTEX_TOKEN": os.environ["LIVE_CORTEX_TOKEN"],
                "CORTEX_SETUP_READY_TIMEOUT_SECS": "15", "CORTEX_BACKUP_MIN_FREE_MB": "32",
                "LIVE_DOCKER_BIN": os.environ["LIVE_DOCKER_BIN"],
                "LIVE_DOCKER_COMPOSE_BIN": os.environ["LIVE_DOCKER_COMPOSE_BIN"],
                "NO_COLOR": "1"})
    marker(1)
    records, failures = [], []
    created = run(binary, ["create"], env)
    if created["exit"] != 0:
        raise RuntimeError(f"managed recovery prerequisite failed: {created['terminal']}")
    stamp = recovery_stamp(created["terminal"])
    # Pin the baseline while restore/rollback retain fresh safety snapshots.
    verify_bundle(stamp, protect=True)
    for entry in entries:
        spelling = entry["spelling"]
        suffix = spelling.removeprefix("setup backup").split()
        help_result = run(binary, [*suffix, "--help"], env)
        checks = [("help", help_result, help_result["exit"] == 0 and help_result["mentions_usage"])]
        tail = backup_args(spelling, stamp)
        if tail == ["schedule", "check"]:
            prerequisite = run(binary, ["schedule", "install"], env)
            if prerequisite["exit"] != 0:
                raise RuntimeError(f"schedule prerequisite failed: {prerequisite['terminal']}")
        if tail[0] in ("restore", "rollback"):
            marker(2)
        positive = run(binary, tail, env)
        ok = positive["exit"] == 0
        if ok and tail[0] == "create":
            produced = recovery_stamp(positive["terminal"])
            verify_bundle(produced)
        if ok and tail[0] in ("restore", "rollback"):
            observed = marker()
            positive["restored_marker"] = observed
            ok = observed == 1 and "authenticated server verification passed" in positive["terminal"]
        if ok and tail[0] == "schedule":
            scheduled = (run_tmp / "backup-crontab").read_text()
            marker_present = "# cortex-backup-" in scheduled and str(home) in scheduled
            ok = not marker_present if tail[1] == "remove" else marker_present
        checks.append(("semantic-positive", positive, ok))
        negative = run(binary, [*suffix, "--cortex-live-invalid-option"], env)
        refused = negative["exit"] != 0 and not negative.get("timeout") and any(
            word in negative["terminal"].lower() for word in ("unknown", "requires", "invalid"))
        checks.append(("validation-negative", negative, refused))
        for case, observation, passed in checks:
            records.append({"surface_id": entry["id"], "case_kind": case,
                            "execution_mode": "executed-managed-recovery-semantic",
                            "result": "pass" if passed else "fail", "observation": observation})
            if not passed:
                failures.append(f"{entry['id']}: {case}")
        for alias in entry["aliases"]:
            # Backup currently has no aliases. Fail closed if that contract changes.
            raise RuntimeError(f"backup alias requires an explicit recovery case: {alias}")
    Path(destination).write_text(json.dumps({"results": records, "failures": failures}, indent=2) + "\n")
    os.chmod(destination, 0o600)
    # cli_sweep merges these observations and emits redacted failure diagnostics.
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
