---
title: "Cortex skills"
created: 2026-07-30
updated: 2026-09-27
---

<!--
SPDX-License-Identifier: MIT
Author: jmagar
License: MIT
Description: Skill definitions and validation guidance for the cortex plugin.
-->

# Cortex skills

The package lives in `plugins/cortex/skills/`. Read its scoped [AGENTS.md](../../plugins/cortex/AGENTS.md) before changing onboarding or runtime instructions. The directory contents and validation scripts, not an old copied tree, define what ships.

## Entry skills

| Skill | Role |
| --- | --- |
| `install-cortex` | Install/onboard through the binary-owned setup flow; distinguish server and client-only roles |
| `using-cortex` | Discover current tools and shared operating guidance; detailed reference in `references/operations.md` |
| `cortex-snippets` | Labby snippet migration entrypoints and bounded workflows |

The old `plugins/cortex/skills/cortex/` directory has been renamed to `using-cortex`. Update include paths and tests when moving skill content; a rename must not silently remove instructions from compiled runtime prompts.

## Specialized skills

The package retains `troubleshoot`, `report`, `logs`, `version-check`, `incidents`, `topology`, `searching-sessions`, `frustration-assessment`, `mcp-friction-assessment`, `hook-friction-assessment`, and `skill-improvement-assessment` during snippet migration. Do not remove them until replacement entrypoints are verified.

Several specialized `SKILL.md` files are intentionally thin and delegate detailed procedure to `references/workflow.md`. Runtime assessment code that embeds a skill must include the referenced workflow as well; read `src/assessment.rs`, `src/mcp_assessment.rs`, and `src/skill_assessment.rs` before refactoring their instruction files.

LLM-backed assessment execution remains an explicit CLI operation. MCP investigations return bounded deterministic evidence; a skill description is not permission to bypass runtime scope or confirmation rules.

## Authoring and validation

Use frontmatter with a stable name and useful description, retain detailed instructions in a reachable reference, and provide `agents/openai.yaml` for new cross-provider skills. Keep example tool names, argument schemas, bounds, and response handling aligned with the live registry and checked-in tests.

```bash
just validate-plugin
just validate-skills
```

`validate-skills` aliases the package validation. Verify package contracts and disposable Skills CLI installation when changing distribution/onboarding behavior. Setup scripts remain thin adapters to `cortex setup`; no Claude Code lifecycle hooks ship.

See [PLUGINS.md](PLUGINS.md), [HOOKS.md](HOOKS.md), and [TOOLS.md](../mcp/TOOLS.md).
