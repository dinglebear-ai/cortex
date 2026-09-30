---
title: "Plugin output styles"
created: "2026-07-30"
updated: 2026-09-30
---

# Plugin output styles

cortex does not define custom output styles. Tool responses are returned as JSON text content blocks, which MCP clients render according to their own formatting preferences.

## Response format

All tools return JSON wrapped in MCP text content blocks:

```json
{
  "content": [
    {
      "type": "text",
      "text": "{\"count\": 3, \"logs\": [...]}"
    }
  ]
}
```

## See also

- [tools.md](../../reference/mcp/tools.md) -- tool response shapes
