from pathlib import Path
import shutil
import subprocess
import unittest


class CortexSnippetContractTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("node"), "Node.js is required to execute Code Mode snippets")
    def test_snippet_bodies_with_bounded_mock_evidence(self):
        script = Path(__file__).with_name("snippet_contract.mjs")
        result = subprocess.run(
            ["node", str(script)], capture_output=True, text=True, check=False, timeout=15
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
