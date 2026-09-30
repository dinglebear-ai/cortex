# Technology stack documentation

Follow the root [AGENTS.md](../../../AGENTS.md). The generated [section index](README.md) covers architecture context, prerequisites, and technology choices.

Keep toolchain and dependency statements tied to `rust-toolchain.toml`, `Cargo.toml`, and the selected workspace configuration. Syslog defaults to UDP/TCP 1514; MCP, REST, OTLP, health, and the browser workspace share HTTP 3100. Shared transport does not imply shared credentials. The current [architecture overview](../../architecture/overview.md) is the system map.
