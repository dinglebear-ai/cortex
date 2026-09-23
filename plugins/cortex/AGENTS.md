# Cortex plugin contributor instructions

This package is a client/onboarding surface over Cortex. The binary, setup engine, Compose lifecycle, auth model, storage policy, and collectors remain owned by repository source and docs.

- `skills/install-cortex` orchestrates root `install.sh`, `cortex setup repair`, Compose, and plugin/client configuration. Do not duplicate setup logic in the skill.
- Keep server and client-only roles distinct. One authoritative server owns ingestion/storage; client-only hosts must not start a second receiver/database accidentally.
- Preserve separate MCP and REST credentials. Syslog itself is unauthenticated and must rely on sender CIDR/network controls.
- OAuth is Google/lab-auth. Static MCP bearer is disabled by default in OAuth runtime config; retaining it is an explicit break-glass choice via `CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH=false`.
- Remote exposure, Tailscale, or existing proxy changes require current official docs, verified config backups/checksums, exact proposed changes, and explicit approval.
- Cortex `CORTEX_LLM=codex` is Codex app-server backed. Do not replace it with shell prompt hacks.
- Every new cross-provider skill should carry `agents/openai.yaml`. Validate skill/package contracts and disposable Skills CLI installation before publication.
