#!/usr/bin/env bash
# Validate every tracked-text occurrence of the deprecated transcript-forward
# environment key. This intentionally uses `git grep` without extension filters
# so extensionless config, systemd units, and newly added file types are covered.
set -euo pipefail

python3 - <<'PY'
from __future__ import annotations

import re
import subprocess
import sys

legacy = "CORTEX_AGENT_AI_TRANSCRIPTS"
new = "CORTEX_AGENT_AI_TRANSCRIPT_FORWARD"
validator = "scripts/validate-transcript-forward-env-rename.sh"

result = subprocess.run(
    ["git", "grep", "-n", "-I", legacy, "--", ":!" + validator],
    check=False,
    stdout=subprocess.PIPE,
    text=True,
)
if result.returncode not in (0, 1):
    raise SystemExit(result.returncode)

doc_paths = {
    "docs/contracts/agent-observatory.md",
    "docs/plans/2026-07-31-agent-observatory-implementation.md",
    "docs/plans/agent-observatory/01a-transcript-forward-env-rename.md",
    "docs/plans/agent-observatory/06-production-hardening-and-docs.md",
    "docs/plans/agent-observatory/proof/PROOF.md",
    "docs/research/2026-07-31-agent-observatory.md",
    "docs/specs/agent-observatory.md",
}

def allowed(path: str, line: str) -> bool:
    if path == "Justfile":
        return line.lstrip().startswith("# ENV-004:")
    if path == "src/heartbeat_agent.rs":
        return re.fullmatch(
            r'pub const AI_TRANSCRIPT_FORWARD_LEGACY_ENV: &str = "' + legacy + r'";',
            line.strip(),
        ) is not None
    if path in {
        "src/agent_deploy_tests.rs",
        "src/heartbeat_agent_tests.rs",
        "src/setup/doctor_tests.rs",
        "src/setup/doctor_transcript_forward_tests.rs",
        "src/setup/heartbeat_agent_tests.rs",
    }:
        # Test occurrences must be string fixtures or assertions, never an env!
        # assignment in executable workflow/config syntax.
        stripped = line.strip()
        return ('"' + legacy) in stripped or (legacy + '=') in stripped
    if path == "scripts/test-validate-transcript-forward-env-rename.sh":
        # This harness deliberately injects forbidden occurrences into an
        # isolated repository. Only its fixture/assertion lines may name the
        # legacy key; executable configuration in this repository stays banned.
        stripped = line.strip()
        return (
            ('"' + legacy) in stripped
            or legacy + "=" in stripped
            or "deprecated compatibility alias" in stripped
            or "convenient shorthand" in stripped
        )
    if path in doc_paths:
        lowered = line.lower()
        return any(word in lowered for word in (
            "deprecated", "deprecation", "legacy", "compatibility", "red", "regression",
            "removal", "rename", "generated", "operationally misleading",
        ))
    return False

violations: list[str] = []
occurrences = 0
for record in result.stdout.splitlines():
    path, number, line = record.split(":", 2)
    occurrences += line.count(legacy)
    if not allowed(path, line):
        violations.append(f"{path}:{number}:{line}")

if violations:
    print(f"error: unapproved {legacy} occurrence(s):", file=sys.stderr)
    print("\n".join(violations), file=sys.stderr)
    raise SystemExit(1)

required = {
    "src/heartbeat_agent.rs": new,
    "docs/contracts/agent-observatory.md": new,
}
for path, token in required.items():
    text = open(path, encoding="utf-8").read()
    if token not in text:
        raise SystemExit(f"error: {path} must document/use {token}")

env_example = subprocess.run(
    ["git", "ls-files", "--error-unmatch", ".env.example"],
    check=False,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
).returncode == 0
if env_example and legacy in open(".env.example", encoding="utf-8").read():
    raise SystemExit(f"error: .env.example contains deprecated {legacy}")

print(f"validated {occurrences} approved tracked-text occurrence(s) of {legacy}")
PY
