# Issue #250 Tasks

Parent: https://github.com/dinglebear-ai/cortex/issues/250

## T1. Add MCP tool projection configuration
Status: in-progress
Type: AFK
Blocked by: None
What to build: Add a canonical MCP projection mode with legacy, atomic, and both options. Preserve legacy as the compatibility default.
Acceptance contract:
- Commands: cargo test; cargo clippy; cargo fmt --check
- Behaviors: default configuration exposes the legacy aggregate tool; atomic mode exposes only atomic action tools; both mode exposes both surfaces.
Gates:
- Lenses: configuration compatibility, public MCP contract
- Security: standard
Rollback note: plain revert, no data side effects.
Expected review focus: compatibility defaults and serialization.
Parallelization: serialize with T2 because both touch MCP surface construction.

## T2. Generate focused atomic tools from ACTION_SPECS
Status: todo
Type: AFK
Blocked by: T1
What to build: Project each enabled ACTION_SPECS entry into exactly one atomic MCP descriptor with a focused schema derived from the existing canonical action metadata and schema definitions.
Acceptance contract:
- Commands: cargo test; cargo clippy; cargo fmt --check
- Behaviors: every ACTION_SPECS entry appears exactly once in atomic mode; atomic schemas omit the action discriminator and unrelated parameters; exact ActionInputContract fields and requirements are preserved.
Gates:
- Lenses: schema drift, public MCP contract
- Security: standard
Rollback note: plain revert, no data side effects.
Expected review focus: schema narrowing and drift prevention.
Parallelization: serialize with T1/T3 because these share projection code.

## T3. Dispatch atomic calls through existing handlers with truthful auth and safety metadata
Status: todo
Type: AFK
Blocked by: T2
What to build: Route atomic tools through the existing Cortex action implementations, preserving auth, validation, errors, telemetry, confirmation semantics, and widget behavior. Add per-operation MCP safety annotations from canonical metadata.
Acceptance contract:
- Commands: cargo test; cargo clippy; cargo fmt --check
- Behaviors: equivalent legacy and atomic calls reach the same ActionHandler; denied read/admin operations fail identically; read-only hints are true only for non-mutating actions; write/destructive/idempotency hints are explicit for mutations.
Gates:
- Lenses: authorization boundary, dispatch equivalence, telemetry
- Security: deep (tool authorization and destructive-operation metadata)
Rollback note: plain revert, no persisted migration.
Expected review focus: fail-closed auth and operation-specific annotations.
Parallelization: serialize with T2.

## T4. Add drift tests and document rollout
Status: todo
Type: AFK
Blocked by: T3
What to build: Add coverage for projection counts, schema focus, annotations, legacy/atomic equivalence, invalid inputs, denied operations, and write flows; document projection configuration and compatibility behavior.
Acceptance contract:
- Commands: cargo test; cargo clippy; cargo fmt --check
- Behaviors: registry/projection drift fails tests; representative read/write/invalid/denied flows are covered; documentation explains legacy, atomic, and both rollout modes.
Gates:
- Lenses: test contract, documentation
- Security: standard
Rollback note: plain revert, no side effects.
Expected review focus: acceptance-criteria coverage and docs matching runtime behavior.
Parallelization: starts after T3.
