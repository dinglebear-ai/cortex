---
title: "Plugin channels"
created: "2026-07-30"
updated: 2026-09-30
---

# Plugin channels

cortex does not use channels. It does not send or receive messages from external messaging platforms.

## Why no channels

cortex is a passive data store -- it receives syslog messages via UDP/TCP and answers MCP queries. It does not generate outbound notifications or integrate with messaging services.

For alerting on syslog events, use the `gotify-mcp` plugin to send notifications based on `cortex errors` or `cortex search` results.

## See also

- [tools.md](../../reference/mcp/tools.md) -- tools for querying log data
