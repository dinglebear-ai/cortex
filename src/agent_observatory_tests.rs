use super::{AGENT_OBSERVATORY_PROJECTION_VERSION, AGENT_OBSERVATORY_SCHEMA_VERSION};

#[test]
fn planned_schema_and_projection_versions_are_locked() {
    assert_eq!(AGENT_OBSERVATORY_SCHEMA_VERSION, 47);
    assert_eq!(AGENT_OBSERVATORY_PROJECTION_VERSION, 1);
}

// Compile-time (not runtime) check: the runtime schema must never claim to have
// already applied Agent Observatory migrations that don't exist yet. Both
// constants are `const i64`, so this ordering is provable at compile time —
// asserting it in `const _` catches drift as soon as this `#[cfg(test)]`
// module is compiled (e.g. `cargo test`, `cargo build --tests`), rather than
// only when the `#[test]` above actually executes. NOTE: this module is
// `#[cfg(test)]`-gated, so a plain `cargo build`/`cargo build --release`
// (which never compiles the test target) does not evaluate this assertion —
// it is a test-compile-time guarantee, not a production-build one.
const _: () = assert!(crate::db::KNOWN_SCHEMA_VERSION < AGENT_OBSERVATORY_SCHEMA_VERSION);
