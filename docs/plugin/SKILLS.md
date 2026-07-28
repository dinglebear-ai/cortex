<!--
SPDX-License-Identifier: MIT
Author: jmagar
License: MIT
Description: Skill definitions and validation guidance for the cortex plugin.
-->

# Skill Definitions -- cortex

Patterns for defining skills (domain knowledge modules) within the cortex plugin.

## Directory structure

```
plugins/
  cortex/
    skills/
      cortex/
        SKILL.md           # Skill definition with tool reference and workflows
      troubleshoot/
        SKILL.md           # Connection/ingest/service failure decision tree
      report/
        SKILL.md           # Time-bounded markdown health/log-analysis reports
      redeploy/
        SKILL.md           # Re-run plugin setup hook and verify health
      logs/
        SKILL.md           # Tail/follow cortex service Docker Compose logs
      version-check/
        SKILL.md           # Running container vs local Compose image check
      incidents/
        SKILL.md           # Error-signature ack/unack, notifications, prior incidents
      topology/
        SKILL.md           # map/host_state/fleet_state/correlate/graph queries
      searching-sessions/
        SKILL.md           # AI transcript search and exploration
      frustration-assessment/
        SKILL.md           # Analyze abuse_investigate evidence bundles
      mcp-friction-assessment/
        SKILL.md           # Analyze mcp_investigate evidence bundles
      hook-friction-assessment/
        SKILL.md           # Analyze hook_investigate evidence bundles
      skill-improvement-assessment/
        SKILL.md           # Analyze skill_investigate evidence bundles
      ...
```

## Skills

| Skill | Purpose |
| --- | --- |
| `cortex` | Client-facing documentation for the cortex MCP tool and its action dispatch. |
| `troubleshoot` | Narrow troubleshooting decision tree for MCP, ingest, service, and missing-host issues. |
| `report` | Generate actionable 24-hour markdown reports from cortex MCP evidence. |
| `logs` | Tail or follow cortex service logs from Docker Compose. |
| `version-check` | Check whether the running container matches the local Compose image. `--pull` checks after refreshing the local image; without it, Docker checks only the local cache. |
| `incidents` | Triage unacknowledged error signatures, review recent notifications, find prior incidents, and pull full incident context. |
| `topology` | Answer homelab topology and cross-host correlation questions via `map`/`host_state`/`fleet_state`/`correlate`/`correlate_state`/`graph`. |
| `searching-sessions` | Search and explore AI transcript sessions (Claude Code/Codex/Gemini). |
| `frustration-assessment` | Analyze an `abuse_investigate` evidence bundle into a frustration/abuse report. |
| `mcp-friction-assessment` | Analyze an `mcp_investigate` evidence bundle for MCP tool reliability issues. |
| `hook-friction-assessment` | Analyze a `hook_investigate` evidence bundle for hook reliability issues. |
| `skill-improvement-assessment` | Analyze a `skill_investigate` evidence bundle for skill-quality issues. |

### Contents

`plugins/cortex/skills/cortex/SKILL.md` includes:
- Tool inventory (1 tool: `cortex`, with the current MCP action set described)
- Parameter reference for each tool
- FTS5 query syntax guide
- Common workflow patterns (health check, incident investigation, host onboarding)
- Severity level reference (emerg through debug)

### Validation

```bash
just validate-skills
# Checks skill definitions under plugins/cortex/skills/
```

## Adding a skill

If additional skills are needed:

1. Create `plugins/cortex/skills/<name>/SKILL.md`
2. Add frontmatter with `name` and `description`
3. Document tools, workflows, and examples
4. Run `just validate-skills`

## See also

- [PLUGINS.md](PLUGINS.md) -- plugin manifest references the skill
- [../mcp/TOOLS.md](../mcp/TOOLS.md) -- MCP tool definitions
