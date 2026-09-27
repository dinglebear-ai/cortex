#!/usr/bin/env bash
set -euo pipefail

# Optional root supports isolated regression fixtures without inspecting a host.
repo_root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
python3 - "$repo_root" <<'PY'
import os
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
try:
    paths = subprocess.check_output(
        ["git", "-C", str(root), "ls-files", "--cached", "--others",
         "--exclude-standard", "-z"],
        stderr=subprocess.PIPE,
    ).split(b"\0")
except subprocess.CalledProcessError as exc:
    print("[agent-memory] FAIL — cannot inspect Git checkout", file=sys.stderr)
    sys.exit(exc.returncode or 1)

names = {"AGENTS.md", "CLAUDE.md", "GEMINI.md"}
local_names = {"AGENTS.override.md", "CLAUDE.local.md", "CLAUDE.md.local"}
errors = []
directories = {Path(".")}
for raw_path in paths:
    if raw_path:
        path = Path(os.fsdecode(raw_path))
        if path.name in names:
            directories.add(path.parent)
        if path.name in local_names:
            errors.append(f"{path} must be untracked and Git-ignored")
for relative in sorted(directories, key=str):
    directory = root / relative
    canonical = directory / "AGENTS.md"
    if canonical.is_symlink() or not canonical.is_file():
        errors.append(f"{relative}/AGENTS.md must be a regular canonical file")
    for name in ("CLAUDE.md", "GEMINI.md"):
        alias = directory / name
        if not alias.is_symlink():
            errors.append(f"{relative}/{name} must be a symlink to AGENTS.md")
        elif os.readlink(alias) != "AGENTS.md":
            errors.append(f"{relative}/{name} must point to relative AGENTS.md")
        elif not alias.is_file():
            errors.append(f"{relative}/{name} is a broken link")

    local = directory / "AGENTS.override.md"
    alias = directory / "CLAUDE.local.md"
    if os.path.lexists(local) or os.path.lexists(alias):
        if local.is_symlink() or not local.is_file():
            errors.append(f"{relative}/AGENTS.override.md must be a regular file")
        elif "AGENTS.md" not in local.read_text(encoding="utf-8"):
            errors.append(f"{relative}/AGENTS.override.md must reference shared AGENTS.md")
        if not alias.is_symlink() or os.readlink(alias) != "AGENTS.override.md":
            errors.append(f"{relative}/CLAUDE.local.md must link to AGENTS.override.md")
        for path in (local, alias):
            if subprocess.run(
                ["git", "-C", str(root), "check-ignore", "--quiet", "--no-index", str(path)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            ).returncode != 0:
                errors.append(f"{path.relative_to(root)} must be Git-ignored")

for error in errors:
    print(f"[agent-memory] FAIL — {error}", file=sys.stderr)
if errors:
    sys.exit(1)
print(f"[agent-memory] OK — AGENTS.md is canonical in {len(directories)} directories")
PY
