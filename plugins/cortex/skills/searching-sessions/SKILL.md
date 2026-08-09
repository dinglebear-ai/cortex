---
name: searching-sessions
description: Search and explore AI/Claude/Codex conversation history stored in cortex. **Trigger when:** the user asks to "search sessions", "find conversations", "what did I work on", "session history", "past conversations", "ask history", "when did I last use X", "show recent sessions", "list AI projects", "what tools have I used"; OR when **referencing past work** like "we've done this before", "we fixed something similar", "haven't we implemented this", "this was already working", "why did we do that again", "I thought we already solved this"; OR when **questioning past decisions** like "why did we implement it this way", "what was the reasoning for", "didn't we discuss this already", "this seems familiar"; OR when the user is **under the something was done before** or wants to find context about previous implementations, fixes, or decisions. Use for session search, conversation discovery, past work retrieval, implementation history, or decision archaeology.
---

# Cortex Session Search

Search and explore AI/Claude/Codex conversation history stored in cortex. The skill provides full-text search over session transcripts, question-based history retrieval with system log context, project summaries, usage patterns, and investigation tools for skill/MCP/hook failures.

## Primary Actions

### `sessions` — Browse recent conversations
List AI transcript sessions grouped by project, tool, session, and host. Use for discovery and overview of recent activity.

### `search_sessions` — Full-text search with relevance ranking
FTS5 search over AI transcript rows. Returns grouped session results ranked by relevance. Best for targeted searches when you know what you're looking for.

### `correlate` (query-only) — Question-based history with system context
Pass `query` without `reference_time` to `correlate` to derive the anchor from the top matching AI-transcript session, then correlate nearby system logs around it (severity-gated, host-grouped, same as any other `correlate` call). The matched session comes back as `matched_session`. Use for "what happened when I worked on X" or "how did we solve Y before".

### `project_context` — Full project summary
Complete context for one AI project path: tools, sessions, hosts, counts, and recent representative entries. Use for getting oriented on a project's activity.

### `ai_correlate` — Session events + system logs
Use AI transcript rows as timeline anchors and pull nearby non-AI syslog, Docker, OTLP, and host events. Excludes AI rows from correlated logs so sessions don't self-correlate. Use for "what system events happened around my session work".

### `usage_blocks` — Activity patterns over time
Bucket AI activity into deterministic 5-hour UTC windows. Use for understanding when work happens and identifying patterns.

## Discovery Actions

### `list_ai_projects` — Distinct projects with counts
Enumerate all known AI projects with session and transcript counts. Use for orientation and discovery.

### `list_ai_tools` — Distinct tools with counts
Enumerate all known AI tools (claude, codex, gemini, etc.) with session and transcript counts. Use for understanding tool usage.

## Investigation Actions

### `skill_investigate` — Skill failure deep-dive
Expands skill-usage incidents into deterministic evidence bundles. Accepts a skill name directly. Use for "what went wrong with cortex-troubleshoot" or "show me skill failure incidents".

### `mcp_investigate` — MCP tool failure deep-dive
Expands MCP-usage incidents into evidence bundles, server/tool-first. Use for "what's failing with the cortex MCP server" or "tool timeout incidents".

### `hook_investigate` — Hook failure deep-dive
Expands hook-usage incidents into evidence bundles, hook-first. Use for "hook failures" or "hook timeout issues".

### `abuse_incidents` — Resource abuse detection
Groups AI transcript abuse hits into scored incident candidates. Returns incidents ordered by priority: low, medium, high, critical.

### `abuse_investigate` — Abuse incident deep-dive
Expands abuse incidents into evidence bundles with transcript context and nearby system logs.

## Workflows

### Browse recent sessions
```
mcp__cortex__cortex(action="sessions", limit=30)
```

### Search for specific topics
```
mcp__cortex__cortex(action="search_sessions", query="nginx ssl", limit=10)
```

### Question-based history with system context
```
mcp__cortex__cortex(action="correlate", query="what causes qbittorrent to keep dying?")
```

### Get full project context
```
mcp__cortex__cortex(action="project_context", project="lab")
```

### Correlate session with system logs
```
mcp__cortex__cortex(action="ai_correlate", session_id="abc123", window_minutes=15)
```

### Investigate skill failures
```
mcp__cortex__cortex(action="skill_investigate", skill="cortex-troubleshoot", limit=5)
```

### Usage patterns over time
```
mcp__cortex__cortex(action="usage_blocks", project="cortex", since="7d")
```

### Discover projects and tools
```
mcp__cortex__cortex(action="list_ai_projects")
mcp__cortex__cortex(action="list_ai_tools")
```

### Investigate abuse incidents
```
mcp__cortex__cortex(action="abuse_incidents", limit=10)
mcp__cortex__cortex(action="abuse_investigate", incident_id=123)
```

## Time Windows

`since`/`until`/`from`/`to` accept multiple formats across all actions:

- **Relative**: `30m`, `1h`, `2d`, `90s`, `7d`
- **Keywords**: `now`, `today`, `yesterday`
- **Bare dates**: `2026-06-01`, `2026-06-01 08:30`
- **Full RFC3339**: `2026-06-19T09:30:00Z`

Omit time windows to use each action's default (often the last hour or full history).

## Common Filters

Many actions support optional filters:
- `project` — scope to one AI project path
- `tool` — scope to one AI tool (claude, codex, gemini, etc.)
- `hostname` — scope to one host
- `limit` — cap results (varies by action, often default 10-50)
- `from`/`to` — time window bounds

## FTS5 Query Syntax

`search_sessions` and query-only `correlate` use SQLite FTS5 with porter stemming:

- `AND`, `OR`, `NOT` are uppercase boolean operators
- Quote phrases and hyphenated terms: `"smoke-test"`, `"nginx ssl"`
- Invalid FTS5 syntax returns a database error

## Action Cost Tiers

Start with **cheap** bounded calls, narrow scope with **moderate** actions, reserve **expensive** for specific questions:

- **cheap**: `sessions`, `list_ai_projects`, `list_ai_tools`, `usage_blocks`
- **moderate**: `search_sessions`, `correlate`, `project_context`, `abuse_incidents`
- **expensive**: `ai_correlate`, `skill_investigate`, `mcp_investigate`, `hook_investigate`, `abuse_investigate`

## When to Use Each Action

| User Intent | Best Action | Why |
|-------------|-------------|-----|
| "What did I work on recently?" | `sessions` | Browse overview |
| "Find conversations about X" | `search_sessions` | Targeted FTS5 search |
| "How did we solve X before?" | `correlate` (query-only) | Returns system context too |
| "Tell me about project X" | `project_context` | Full project summary |
| "What happened around that session?" | `ai_correlate` | Session + system events |
| "When do I use tool X?" | `usage_blocks` | Activity patterns |
| "What's failing with skill X?" | `skill_investigate` | Skill failure incidents |
| "Show me projects I've used" | `list_ai_projects` | Discovery |
| "What tools have I used?" | `list_ai_tools` | Discovery |
| "Any abuse incidents?" | `abuse_incidents` | Abuse detection |

## Trigger Scenarios & Action Mapping

### "We've done/fixed this before" → Implementation archaeology
```
# Search for the implementation topic
mcp__cortex__cortex(action="search_sessions", query="database migration", limit=10)

# Or use query-only correlate for system context
mcp__cortex__cortex(action="correlate", query="how did we fix the CORS issue last time")
```

### "This was already implemented" → Verify implementation history
```
# Search for the feature/implementation keywords
mcp__cortex__cortex(action="search_sessions", query="authentication OAuth", limit=10)

# Check project context for the relevant project
mcp__cortex__cortex(action="project_context", project="/home/jmagar/workspace/cortex")
```

### "Why did we do it this way?" → Decision archaeology
```
# Search for discussions around the implementation
mcp__cortex__cortex(action="correlate", query="why did we choose Postgres over MongoDB")

# Correlate with system events from that time
mcp__cortex__cortex(action="ai_correlate", session_id="abc123", window_minutes=30)
```

### "Didn't we already discuss this?" → Conversation retrieval
```
# Search for the topic across all sessions
mcp__cortex__cortex(action="search_sessions", query="rate limiting strategy", limit=10)

# Or ask as a natural question
mcp__cortex__cortex(action="correlate", query="when did we last discuss error handling patterns")
```

### "What was the reasoning for..." → Context recovery
```
# Query-only correlate returns both AI conversation and system logs
mcp__cortex__cortex(action="correlate", query="what was the reasoning for removing the cache layer")
```

### "This seems familiar" → Pattern recognition
```
# Search for similar implementations or discussions
mcp__cortex__cortex(action="search_sessions", query="circuit breaker pattern", limit=10)

# List recent projects to orient yourself
mcp__cortex__cortex(action="list_ai_projects")
```

## CLI Examples

The same actions are available via the CLI with `--json` output:

```bash
# Browse sessions
cortex sessions search "" --limit 30 --json

# Search transcripts
cortex sessions search "nginx ssl" --limit 10 --json

# Question-based history with system context (query-only correlate)
cortex correlate events --query "what causes qbittorrent to keep dying?"

# Project context
cortex sessions context --project /home/jmagar/workspace/cortex --json

# Investigate skill failures
cortex sessions investigate --skill cortex-troubleshoot --limit 5 --json

# Usage blocks
cortex sessions blocks --project /home/jmagar/workspace/cortex --since 7d --json

# List projects
cortex sessions projects --json

# List tools
cortex sessions tools --json
```

## Transport Strategy & CodeMode Patterns

This skill uses a **three-tier fallback** for session search:

### Tier 1: Labby MCP + CodeMode (Efficient Fan-out)

When Labby gateway is available, use CodeMode for parallel searches and batching:

```javascript
// Parallel fan-out: search multiple topics at once
async () => {
  const [rust, docker, config] = await Promise.all([
    callTool("cortex::cortex", {action: "search_sessions", query: "rust", limit: 5}),
    callTool("cortex::cortex", {action: "search_sessions", query: "docker", limit: 5}),
    callTool("cortex::cortex", {action: "search_sessions", query: "config", limit: 5})
  ]);
  return {rust: rust.sessions?.length, docker: docker.sessions?.length, config: config.sessions?.length};
}
```

```javascript
// Dependent calls: list projects, then get context for each
async () => {
  const projects = await callTool("cortex::cortex", {action: "list_ai_projects"});
  const top3 = projects.projects?.slice(0, 3) || [];
  const contexts = await Promise.all(
    top3.map(p => callTool("cortex::cortex", {
      action: "project_context",
      project: p.project,
      limit: 10
    }))
  );
  return contexts;
}
```

```javascript
// Batch investigation: check multiple skills for incidents
async () => {
  const skills = ["cortex-troubleshoot", "cortex-dr", "cortex-logs"];
  const incidents = await Promise.all(
    skills.map(skill => callTool("cortex::cortex", {
      action: "skill_investigate",
      skill: skill,
      limit: 3
    }))
  );
  return incidents.filter(r => r.incidents?.length > 0);
}
```

### Tier 2: Cortex MCP Tool

When Cortex MCP tool is available (via plugin HTTP connection):

```javascript
mcp__cortex__cortex({
  action: "search_sessions",
  query: "nginx",
  limit: 10
})
```

### Tier 3: Cortex CLI (Fallback)

When MCP tools are unavailable, use CLI commands directly:

```bash
cortex sessions search "query" --limit N --json
cortex sessions context --project /path/to/project --json
cortex sessions projects --json
cortex sessions tools --json
```

## CLI Output Parsing (Tier 3)

CLI responses with `--json` return structured data:

```json
{
  "total_candidates": 600,
  "candidate_rows": 5000,
  "truncated": true,
  "sessions": [
    {
      "session_key": "6:devhost|6:claude|29:/home/jmagar/workspace/cortex|36:SESSION_ID",
      "project": "/home/jmagar/workspace/cortex",
      "tool": "claude",
      "session_id": "...",
      "hostname": "devhost",
      "first_seen": "2026-07-08T06:50:01.053Z",
      "last_seen": "2026-07-08T06:51:11.235Z",
      "event_count": 152,
      "match_count": 7,
      "best_snippet": "..."
    }
  ]
}
```
