#!/usr/bin/env python3
"""Hermetic restore contract: optional signing key and failed staging."""
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest

HELPER = Path(__file__).resolve().with_name("restore-backup.sh")


class RestoreTests(unittest.TestCase):
    def test_optional_credential_key_is_restored_and_failed_copy_preserves_live_files(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            backups, data = root / "backups", root / "data"
            backups.mkdir()
            data.mkdir()
            with sqlite3.connect(backups / "syslog-test.db") as conn:
                conn.execute("CREATE TABLE canary (value TEXT)")
                conn.execute("INSERT INTO canary VALUES ('backup')")
            key = backups / "integration-credential-test.key"
            key.write_text("restored-secret")
            env = dict(os.environ, PATH="/usr/bin:/bin:" + os.environ.get("PATH", ""))
            subprocess.run(["sh", str(HELPER), str(backups), "test", str(data)], env=env, check=True)
            self.assertEqual((data / "integration-credential.key").read_text(), "restored-secret")
            before = (data / "cortex.db").read_bytes()
            (data / "cortex.db-wal").write_text("preserve-sidecar")
            key.unlink()
            key.mkdir()  # cp without -r must fail while staging the optional key.
            result = subprocess.run(["sh", str(HELPER), str(backups), "test", str(data)], env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((data / "cortex.db").read_bytes(), before)
            self.assertEqual((data / "integration-credential.key").read_text(), "restored-secret")
            self.assertEqual((data / "cortex.db-wal").read_text(), "preserve-sidecar")


if __name__ == "__main__":
    unittest.main()
