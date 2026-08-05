//! Canonical entity resolution: key grammar, observations, and deterministic
//! resolver decisions for the investigation graph.
//!
//! This module owns the hard-break canonical service identity contract:
//! `logical_service:plex` for logical identity and
//! `service_instance:nashost/plex` for host-scoped deployment topology.
//! Legacy nested shapes (`nashost:plex`, `nashost:plex:plex`, `plex/plex/plex`)
//! are classified for rejection, never normalized.

pub mod adapters;
pub mod observation;
pub mod resolver;
pub mod vocab;

pub use adapters::*;
pub use observation::*;
pub use resolver::*;
pub use vocab::*;

#[cfg(test)]
#[path = "entity_resolution_tests.rs"]
mod tests;
