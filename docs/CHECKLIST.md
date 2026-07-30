---
title: "Release Audit Checklist -- cortex"
created: 2026-04-04
updated: 2026-07-30
---

# Release Audit Checklist -- cortex

Supplemental pre-release audit checklist. `docs/RELEASE.md` is the source of
truth for hermetic and live release gates.

## Version and metadata

- [ ] Version-bearing files in sync: `Cargo.toml`, `Cargo.lock`,
      `server.json`, `mcpb/manifest.json`, and `CHANGELOG.md`
- [ ] Plugin manifests are unversioned:
      `.claude-plugin/plugin.json` and `plugins/**/plugin.json`
- [ ] `CHANGELOG.md` has an entry for the new version
- [ ] README version badge is correct

## Configuration

- [ ] `.env.example` documents every environment variable the server reads
- [ ] `.env.example` has no actual secrets -- only placeholders
- [ ] `.env` is in `.gitignore` and `.dockerignore`

## Documentation

- [ ] `CLAUDE.md` is current and matches repo structure
- [ ] `README.md` has up-to-date tool reference and environment variable table
- [ ] `plugins/cortex/skills/cortex/SKILL.md` has correct frontmatter and tool descriptions
- [ ] Setup instructions work from a clean clone

## Security

- [ ] No credentials in code, docs, or git history
- [ ] `.gitignore` includes `.env`, `*.secret`, credentials files
- [ ] `.dockerignore` includes `.env`, `.git/`, `*.secret`

- [ ] `/health` endpoint is unauthenticated; `/mcp` requires bearer auth when `CORTEX_TOKEN` is set
- [ ] Container runs as non-root (UID 1000)
- [ ] No baked environment variables in Docker image
- [ ] Bearer token comparison uses constant-time equality (`subtle::ConstantTimeEq`)

## Build and test

- [ ] Docker image builds: `docker compose build`
- [ ] Docker healthcheck passes against the intended deployment
- [ ] CI pipeline passes the hermetic gates in `docs/RELEASE.md`
- [ ] Live smoke test passes: `just test-live`
- [ ] `cargo clippy --all-targets -- -D warnings` produces zero warnings

## Deployment

- [ ] `docker-compose.yml` uses correct ports (1514 UDP/TCP, 3100 TCP)
- [ ] `cortex compose doctor` passes before lifecycle mutations
- [ ] Reverse proxy config tested when exposing the service externally

## Registry (if publishing)

- [ ] `server.json` for MCP registry is valid JSON with correct version
- [ ] `mcpb/manifest.json` is valid JSON with matching package metadata
- [ ] OCI image published to `ghcr.io/dinglebear-ai/cortex`
- [ ] Crate published to crates.io (if applicable)
- [ ] DNS verification for `tv.tootie/cortex`

## Marketplace (if applicable)

- [ ] Entry in the active plugin marketplace manifest is current
- [ ] Plugin installs correctly from the current marketplace source
