from pathlib import Path
import unittest

PLUGIN = Path(__file__).resolve().parents[2] / "install-cortex"
ROOT = PLUGIN.parents[1]
SKILL = PLUGIN / "skills" / "install-cortex" / "SKILL.md"
REF = PLUGIN / "skills" / "install-cortex" / "references" / "setup.md"
OPENAI = PLUGIN / "skills" / "install-cortex" / "agents" / "openai.yaml"

class InstallCortexContractTest(unittest.TestCase):
    def test_install_and_security_contract(self):
        text = SKILL.read_text() + REF.read_text()
        required = [
            "cortex setup start", "cortex setup verify", "cortex setup effective", "CORTEX_ALLOWED_SOURCE_CIDRS",
            "CORTEX_TOKEN", "CORTEX_API_TOKEN", "auth/google/callback",
            "CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH=false",
            "CORTEX_LLM=codex", "Codex app-server", "restart: unless-stopped",
            "explicit approval",
        ]
        for value in required:
            self.assertIn(value, text)

    def test_skill_requires_explicit_invocation(self):
        self.assertIn("allow_implicit_invocation: false", OPENAI.read_text())

    def test_oauth_setup_preserves_secure_static_token_default(self):
        expected = 'CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH="${CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH:-true}"'
        insecure = 'CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH="${CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH:-false}"'
        for script in [ROOT / "scripts" / "plugin-setup.sh", ROOT / "plugins" / "cortex" / "scripts" / "plugin-setup.sh"]:
            text = script.read_text()
            self.assertIn(expected, text)
            self.assertNotIn(insecure, text)

    def test_canonical_installer_delegates_to_role_aware_setup(self):
        text = (ROOT / "install.sh").read_text()
        self.assertIn("dinglebear-ai/cortex", text)
        self.assertIn('"$BIN" setup start "$@"', text)
        self.assertIn("checksum mismatch", text)

if __name__ == "__main__":
    unittest.main()
