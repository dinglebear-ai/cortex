//! Agent Observatory domain constants and coordination entry points.

pub mod identity;

/// Schema version reached after all planned Agent Observatory migrations.
///
/// This is intentionally separate from `db::KNOWN_SCHEMA_VERSION` until
/// migrations 44 through 47 are implemented and verified.
pub const AGENT_OBSERVATORY_SCHEMA_VERSION: i64 = 47;

/// Version of the durable Agent Observatory projection contract.
pub const AGENT_OBSERVATORY_PROJECTION_VERSION: u32 = 1;

#[cfg(test)]
#[path = "agent_observatory_tests.rs"]
mod tests;
