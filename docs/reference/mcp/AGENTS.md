# MCP documentation

Follow the root [AGENTS.md](../../../AGENTS.md). This is the canonical instruction source for MCP documentation; its Claude and Gemini aliases point here. The generated [section index](README.md) lists every guide, including prompts and headless evaluation.

`src/mcp/actions.rs` owns action names, scopes, flags, and handlers. Runtime schemas derive from that registry; [tools.md](tools.md) and [schema.md](schema.md) are maintained explanations with drift tests, not generated inventories. Preserve the distinction between deterministic MCP investigation and explicit CLI-only LLM assessments.

Keep transport, authentication, client connection, resources, prompts, UI, and testing guidance aligned with source. Update the matching contract tests when a documented path or filename moves. Do not duplicate the generated section inventory here.
