---
title: "Output Style Definitions -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# Output Style Definitions -- cortex

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

- [../mcp/TOOLS.md](../mcp/TOOLS.md) -- tool response shapes
