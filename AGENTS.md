# Cortex

Cortex is a Rust observability service that combines syslog, OTLP telemetry, host inventory, and AI-session evidence in SQLite, with MCP, REST, CLI, and a browser investigation workspace.

## Where changes belong

Shared query behavior belongs in `CortexService` under `src/app/`; persistence, migrations, and evidence projections belong in `src/db/`. Runtime wiring is in `src/runtime*`. The [architecture guide](docs/architecture.md) maps the remaining subsystems.

`ACTION_SPECS` in [src/mcp/actions.rs](src/mcp/actions.rs) owns MCP action names, scopes, flags, and handlers. The public tool is `cortex`, dispatched by `action`. CLI/REST paths and aliases also use `src/surfaces/`; the surfaces are related, not identical. See [MCP tools](docs/mcp/TOOLS.md) and [API contracts](docs/api.md) when changing an exposed operation.

Transcript formats and evidence lanes are described by `src/scanner/providers.rs`; parsing remains provider-local. Adapter support is not proof of observed delivery, and unknown coverage is not zero events. Use [Adding sources](docs/ADDING_SOURCES.md) before extending collection. The host-local cortex agent is the preferred Docker-log path; `src/docker_ingest/` retains legacy central pull compatibility.

The investigation graph and Agent Observatory are evidence-derived projections, not separate event-history authorities. LLM-backed assessments remain explicit CLI operations; MCP investigations return deterministic evidence.

## Build and checks

The Cargo workspace contains the `cortex` library/binary and `xtask`. `rust-toolchain.toml` selects the compiler; `Cargo.toml` pins the MCP SDK and auth dependency. Cargo output defaults to `.cache/cargo/`, not `target/`.

```bash
cargo build --locked
env -u CORTEX_API_TOKEN -u NO_AUTH cargo nextest run --workspace --locked
cargo test --workspace --doc --locked
cargo xtask pre-push
```

`Justfile` loads `.env` for every recipe. Its test/coverage recipes remove ambient auth variables deliberately; preserve that behavior when adding test commands. `just release` also runs `link-bin`: use `cargo build --release --locked` for a build that should not replace the installed CLI. Full gates and coverage commands are in [CONTRIBUTING.md](CONTRIBUTING.md).

The browser package is `web/`, with its package manager pinned in `web/package.json`. Its `lint`, `typecheck`, `test`, and `build` scripts are invoked with `pnpm --dir web`; `web/out/` is the generated static export served by `src/web_app.rs`. Live profiles have separate prerequisites and target grants: [LIVE_QUALIFICATION.md](docs/LIVE_QUALIFICATION.md).

## Runtime distinctions

`cortex serve mcp` owns server ingestion and maintenance. `cortex mcp` is local query-only stdio. CLI queries use REST when HTTP mode is selected; otherwise they use the configured local database. Routing lives in `src/cli/run.rs`, including `CORTEX_USE_HTTP`, `--http`, and `--server`.

Defaults are syslog UDP/TCP 1514 and HTTP 3100. The shared HTTP port does not imply shared credentials: `CORTEX_TOKEN` is for MCP/machine ingest where applicable, `CORTEX_API_TOKEN` is required for REST, and `CORTEX_API_ADMIN_TOKEN` covers privileged REST. Auth-policy exceptions are documented in [SECURITY.md](docs/SECURITY.md).

A local server needs a writable DB path; the built-in `/data/cortex.db` default is container-oriented. SQLite uses WAL. Retention aging, logical DB-size deletion, and low-free-disk write blocking are different policies; err+ age exemptions do not guarantee immunity from storage-pressure cleanup. See [CONFIG.md](docs/CONFIG.md) and the [retention contract](docs/contracts/retention-policy.md).

Installed Compose ownership is resolved by `cortex compose doctor` and `cortex compose status --json`, not inferred from the source checkout. Host-agent forwarding uses the implemented HTTP ingest endpoints; HTTP MCP uses stateless `POST /mcp`.

## Packaging and documentation

The onboarding/skills package is `plugins/cortex/`; its [scoped instructions](plugins/cortex/AGENTS.md) cover package changes. Entry skills are `install-cortex`, `using-cortex`, and `cortex-snippets`. Embedded assessment prompts must retain both the thin `SKILL.md` and its referenced workflow. The binary owns setup; plugin scripts are adapters, and no Claude lifecycle hooks ship. Run `just validate-plugin` for package changes.

Release-please manages version PRs; `release/components.toml` defines synchronized carriers. Plugin manifests are unversioned and the root Cargo package is not published to crates.io. See [RELEASING.md](RELEASING.md).

[docs/README.md](docs/README.md) indexes current guides. [Documentation maintenance](docs/repo/DOCUMENTATION.md) covers scoped instruction aliases, private overrides, frontmatter, and repository checks. The npm package README mirrors the root README: after editing it, run `node packages/cortex-rmcp/scripts/sync-readme.js` and the package checks.
