# Releasing Cortex

The maintained [release checklist](docs/development/release.md) describes local, CI, and explicitly authorized live gates. The [release audit checklist](docs/development/checklist.md) covers the supplemental operator review.

Normal changes use Conventional Commits and do not bump versions by hand. Release-please prepares release PRs after green main CI; `release/components.toml` owns the synchronized version carriers. `cargo xtask check-version-sync` checks their agreement, and `cargo xtask check-release-versions` adds the release-specific checks. See [publishing and distribution](docs/reference/mcp/publish.md) for native, npm, container, and MCP Registry packaging.

This root entrypoint is retained for existing contributor links. Edit the maintained guides rather than duplicating their command inventories here.
