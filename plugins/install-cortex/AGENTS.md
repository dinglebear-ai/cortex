# Cortex installer plugin

Read the repository root AGENTS.md. This package owns `install-cortex` and the MCP client configuration. Delegate setup and deployment to the canonical installer and Cortex binary. Keep server and client-only roles distinct, preserve separate MCP and REST credentials, and require the documented approval for proxy or exposure changes. Do not add lifecycle hooks. Run `just validate-plugin` after changes.
