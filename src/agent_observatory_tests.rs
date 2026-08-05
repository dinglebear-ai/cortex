use super::{AGENT_OBSERVATORY_PROJECTION_VERSION, AGENT_OBSERVATORY_SCHEMA_VERSION};

#[test]
fn planned_schema_and_projection_versions_are_locked() {
    assert_eq!(AGENT_OBSERVATORY_SCHEMA_VERSION, 47);
    assert_eq!(AGENT_OBSERVATORY_PROJECTION_VERSION, 1);
}

// Compile-time (not runtime) check: the runtime schema must never claim to have
// already applied Agent Observatory migrations that don't exist yet. Both
// constants are `const i64`, so this ordering is provable at compile time —
// asserting it in `const _` catches drift at build time instead of only when
// `cargo test` happens to run.
const _: () = assert!(crate::db::KNOWN_SCHEMA_VERSION < AGENT_OBSERVATORY_SCHEMA_VERSION);
