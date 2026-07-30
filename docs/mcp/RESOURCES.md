---
title: "MCP Resources Reference -- cortex"
created: "2026-07-30"
updated: "2026-07-30"
---

# MCP Resources Reference -- cortex

## Overview

MCP resources expose read-only data via URI-based access. Unlike tools, resources do not perform mutations -- they return the current state of a data source.

## Available resources

cortex exposes one MCP resource:

| URI | Description | MIME type |
| --- | --- | --- |
| `cortex://schema/mcp-tool` | JSON schema for the `cortex` MCP tool and action-based parameters | `application/json` |

All log data access is through the single `cortex` MCP tool. Tools are preferred
over log resources because queries benefit from parameterized filtering
(hostname, severity, source identity, time range, FTS5 query, and correlation
windows) that URI templating cannot express efficiently.

## Future considerations

If log data resources are added in the future, they would use the `cortex://`
URI scheme:

```
cortex://stats           # Database statistics
cortex://hosts           # Host registry
cortex://hosts/{name}    # Logs for a specific host
```

## See also

- [TOOLS.md](TOOLS.md) -- MCP tool reference
