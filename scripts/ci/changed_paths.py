#!/usr/bin/env python3
"""Classify changed files into Cortex CI routing categories."""

from __future__ import annotations

import argparse
import os
import subprocess
from collections.abc import Callable
from pathlib import Path


OUTPUT_KEYS = [
    "all",
    "docs",
    "workflow",
    "rust",
    "web",
    "docker",
    "release",
    "skills",
    "security",
    "mcp",
]


def starts(path: str, *prefixes: str) -> bool:
    return any(path == prefix.rstrip("/") or path.startswith(prefix) for prefix in prefixes)


def any_match(paths: list[str], predicate: Callable[[str], bool]) -> bool:
    return any(predicate(path) for path in paths)


def classify(event: str, paths: list[str]) -> dict[str, bool]:
    if event in {"schedule", "workflow_dispatch"}:
        return {key: True for key in OUTPUT_KEYS}

    if not paths:
        return {key: True for key in OUTPUT_KEYS}

    workflow = any_match(
        paths,
        lambda p: starts(p, ".github/workflows/")
        or p
        in {
            "scripts/ci/changed_paths.py",
            "tests/ci_changed_paths.rs",
        },
    )
    docs = any_match(paths, lambda p: starts(p, "docs/") or p == "README.md")
    web = any_match(paths, lambda p: starts(p, "web/"))
    docker = web or any_match(
        paths,
        lambda p: starts(p, "config/")
        or p
        in {
            ".dockerignore",
            "config/Dockerfile",
            "docker-compose.yml",
            "docker-compose.prod.yml",
        },
    )
    skills = any_match(paths, lambda p: starts(p, "plugins/cortex/skills/", ".claude-plugin/"))
    rust = any_match(
        paths,
        lambda p: starts(p, "src/", "tests/", "xtask/", ".cargo/", "scripts/")
        or p in {"Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml"},
    )
    mcp = any_match(
        paths,
        lambda p: starts(p, "src/mcp/", "docs/mcp/", "docs/reference/mcp/")
        or p in {"tests/test_live.sh", "config/mcporter.json"},
    )
    release = rust or web or skills or any_match(
        paths,
        lambda p: starts(p, "release/", "packages/cortex-rmcp/")
        or p
        in {
            "CHANGELOG.md",
            "install.sh",
            "install.ps1",
            "server.json",
            "mcpb/manifest.json",
            "docker-compose.prod.yml",
        },
    )
    security = rust or any_match(
        paths,
        lambda p: starts(p, ".cargo/", "vendor/") or p in {"Cargo.lock", "deny.toml"},
    )

    result = {
        "all": False,
        "docs": docs,
        "workflow": workflow,
        "rust": rust,
        "web": web,
        "docker": docker,
        "release": release,
        "skills": skills,
        "security": security,
        "mcp": mcp,
    }

    if workflow:
        for key in OUTPUT_KEYS:
            result[key] = True

    return result


def read_paths(path: Path) -> list[str]:
    if not path.exists():
        return []
    return [line.strip() for line in path.read_text().splitlines() if line.strip()]


def git_path_exists(rev: str) -> bool:
    return subprocess.run(
        ["git", "cat-file", "-e", rev],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def git_output(*args: str) -> str:
    return subprocess.check_output(["git", *args], text=True, stderr=subprocess.DEVNULL).strip()


def resolve_paths(event: str) -> list[str]:
    if event in {"schedule", "workflow_dispatch"}:
        return []

    env = os.environ
    base = ""
    head = env.get("HEAD_SHA") or env.get("GITHUB_SHA") or "HEAD"

    if event == "pull_request":
        base = env.get("PR_BASE_SHA", "")
        head = env.get("PR_HEAD_SHA") or head
    elif event == "push":
        if env.get("GITHUB_REF", "").startswith("refs/tags/"):
            return []
        base = env.get("PUSH_BEFORE_SHA", "")
    else:
        return []

    if not base or set(base) == {"0"} or not git_path_exists(base):
        try:
            base = git_output("rev-parse", "HEAD^")
        except subprocess.CalledProcessError:
            base = ""

    if not base:
        return []

    try:
        raw = git_output("diff", "--name-only", base, head)
    except subprocess.CalledProcessError:
        return []

    return [line.strip() for line in raw.splitlines() if line.strip()]


def write_outputs(path: Path, values: dict[str, bool]) -> None:
    lines = [f"{key}={'true' if values[key] else 'false'}" for key in OUTPUT_KEYS]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event", required=True)
    parser.add_argument("--changed-files", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write-changed-files", type=Path)
    args = parser.parse_args()

    paths = read_paths(args.changed_files) if args.changed_files else resolve_paths(args.event)
    if args.write_changed_files:
        args.write_changed_files.write_text("\n".join(paths) + ("\n" if paths else ""))

    values = classify(args.event, paths)
    write_outputs(args.output, values)
    for key in OUTPUT_KEYS:
        print(f"{key}={str(values[key]).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
