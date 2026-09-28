# Changelog

## Unreleased

- Switch the `cortex-topology` snippet from full inventory snapshots to bounded host and app activity queries so it can return useful evidence without a slow inventory fetch; label the reduced coverage explicitly.
- Bound skill assessment and session-search snippet output, scope session search to one hour by default, and add executable snippet contract checks. Graph dependency questions still use the direct Cortex tool.
- Rename the primary skill to `using-cortex` and add `cortex-snippets` with eleven validated Labby Code Mode source snippets. Keep specialized skills available during migration; service-log following and image identity checks still require host-local commands.

## 0.1.0 - 2026-09-18

- Add `install-cortex` as the first-class server/client installation and repair workflow.
- Cover canonical installer/setup repair, Compose persistence, syslog network controls, separate MCP/REST credentials, Google OAuth with optional static bearer break-glass, reverse-proxy/Tailscale approval gates, Codex app-server LLM support, and live client verification.
- Add package-level README and contributor instructions for the existing Cortex plugin.
