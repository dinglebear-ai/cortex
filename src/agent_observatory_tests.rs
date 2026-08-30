use super::{AGENT_OBSERVATORY_PROJECTION_VERSION, AGENT_OBSERVATORY_SCHEMA_VERSION};

#[test]
fn planned_schema_and_projection_versions_are_locked() {
    assert_eq!(AGENT_OBSERVATORY_SCHEMA_VERSION, 48);
    assert_eq!(AGENT_OBSERVATORY_PROJECTION_VERSION, 1);
}

const _: () = assert!(crate::db::KNOWN_SCHEMA_VERSION >= AGENT_OBSERVATORY_SCHEMA_VERSION);
