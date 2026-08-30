use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::app::hook_incident_findings;
use crate::app::incident_findings;
use crate::app::mcp_incident_findings;
use crate::app::skill_incident_findings;
use crate::db;

mod ai_hook_incidents;
mod ai_incidents;
mod ai_inventory;
mod ai_mcp_incidents;
mod ai_sessions;
mod ai_skill_incidents;
mod artifact_evidence;
mod context;
mod core;
mod graph;
mod hook_assess;
mod hook_events;
mod investigation;
mod log_query;
mod mcp_assess;
mod mcp_events;
mod ops;
mod rag;
mod session_pages;
mod skill_assess;
mod skill_events;
mod stats;

pub use ai_hook_incidents::*;
pub use ai_incidents::*;
pub use ai_inventory::*;
pub use ai_mcp_incidents::*;
pub use ai_sessions::*;
pub use ai_skill_incidents::*;
pub use artifact_evidence::*;
pub use context::*;
pub use core::*;
pub use graph::*;
pub use hook_assess::*;
pub use hook_events::*;
pub use investigation::*;
pub use log_query::*;
pub use mcp_assess::*;
pub use mcp_events::*;
pub use ops::*;
pub use rag::*;
pub use session_pages::*;
pub use skill_assess::*;
pub use skill_events::*;
pub use stats::*;

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
