# Repository structure

See [AGENTS.md](../../AGENTS.md) for the current module ownership map and [architecture.md](../architecture.md) for data flow. This map lists stable areas rather than pretending to enumerate every source file.

```text
cortex/
├── AGENTS.md                  # canonical instructions
├── CLAUDE.md -> AGENTS.md      # compatibility alias
├── GEMINI.md -> AGENTS.md      # compatibility alias
├── CONTRIBUTING.md            # development and validation workflow
├── Cargo.toml / Cargo.lock    # cortex package and workspace dependencies
├── rust-toolchain.toml        # pinned compiler and components
├── .cargo/config.toml         # xtask alias and .cache/cargo output
├── src/                       # library, binary, adapters, services, storage
├── xtask/                     # version and pre-push automation
├── tests/                     # integration, live qualification, fixtures
├── web/                       # browser workspace source and tests
├── plugins/cortex/            # onboarding package and skills
├── packages/cortex-rmcp/       # npm distribution wrapper
├── mcpb/                      # MCP bundle metadata
├── release/                   # version-carrier and release contracts
├── .github/workflows/         # CI, release, registry, live qualification
├── config/                    # client/service/forwarder configuration
├── deploy/                    # deployment templates and policies
├── docker-compose.yml         # development Compose configuration
├── docker-compose.prod.yml    # release-image Compose configuration
├── scripts/                   # validators, build/setup, maintenance helpers
├── docs/                      # current guides, contracts, dated history
├── Justfile                   # supported task recipes
├── lefthook.yml               # staged-file and path-aware push gates
├── server.json                # MCP registry metadata
├── RELEASING.md               # release process
├── LICENSE                    # repository AGPL-3.0-only license
└── LICENSING.md                # licensing policy and commercial option
```

Generated `.cache/cargo/`, browser build output, runtime data, logs, and ignored local credentials are not repository source. `git status --short` and `.gitignore` determine what will actually be committed; do not force-add generated/runtime files.

## Source organization

`src/main.rs` is the binary entrypoint; `src/lib.rs` exports the shared library. `src/runtime*` wires the runtime; `src/app/` owns `CortexService`; `src/db/` owns persistence. `src/receiver*` is the syslog receiver (there is no current `src/syslog.rs` receiver module). `src/scanner*`, `src/agent*`, inventory, OTLP, and file-tail modules handle other source families.

Rust modules use sibling `foo.rs` plus `foo/`, with sidecar `*_tests.rs` files. Read the nearest scoped `AGENTS.md` before editing.

## Documentation areas

`docs/README.md` indexes maintained guides. `docs/contracts/` and `contracts/` contain human and machine-readable contracts; inspect status and implementation references rather than assuming all proposals are delivered. `docs/repo/` covers contribution tooling. Dated plans, research, reviews, reports, and sessions preserve historical context.

## Plugin boundaries

`plugins/cortex/` is the client/onboarding package. Skills live under `plugins/cortex/skills/`, including the current entry skills `install-cortex`, `using-cortex`, and `cortex-snippets`. Setup scripts delegate to the Cortex binary; the package does not register Claude Code lifecycle hooks. See its scoped [instructions](../../plugins/cortex/AGENTS.md).
