# Justfile recipes

Run `just --list` for the current catalog. The root `Justfile` is executable authority; this page groups commonly used recipes without copying their entire implementations.

| Area | Recipes |
| --- | --- |
| Development | `just dev`, `build`, `release`, `check`, `lint`, `fmt` |
| Hermetic tests | `just test` (cargo-nextest), `just test-doc` (doctests) |
| Coverage | `just coverage`, `just coverage-html` (cargo-llvm-cov + nextest) |
| Plugin validation | `just validate-plugin`; `validate-skills` is its alias |
| Container build | `just docker-build`, using `config/Dockerfile` |
| Compose lifecycle | `just up`, `down`, `restart`, `logs` |
| Diagnostics | `just health`, `runtime-current`; use guarded `cortex compose doctor` for installed ownership |
| Live qualification | `just test-live` / `live-smoke`, plus named `live-*` profiles |
| Bundle packaging | `just build-mcpb`, `build-mcpb-windows` |
| Setup helpers | `just setup`, `gen-token`, `install`, `link-bin` |

Bare recipe names in the table are invoked with `just`. `just test` unsets selected ambient auth variables before nextest; coverage also removes an ambient DB-path override. See [CONTRIBUTING.md](../../CONTRIBUTING.md) for the complete local gates.

`just dev` starts a service and needs writable storage and valid configuration. Compose lifecycle commands mutate the resolved project; a recipe is not permission to restart a production deployment. Live profiles require their documented prerequisites and, for fleet/provider targets, explicit grants. See [LIVE_QUALIFICATION.md](../LIVE_QUALIFICATION.md).

## Release commands

Normal releases use release-please, not a version bump on every feature push. `cargo xtask` owns synchronization of files declared in `release/components.toml`. `just publish [major|minor|patch]` is an explicit manual escape hatch that commits, tags, and pushes from clean `main`; it does not add versions to intentionally unversioned plugin manifests. See [RELEASING.md](../../RELEASING.md).
