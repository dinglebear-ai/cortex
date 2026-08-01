//! Fleet SSH inventory — the collection side of the inventory/graph
//! sub-product.
//!
//! Collectors gather homelab state from local Compose/proxy files, SSH
//! sessions to fleet hosts (strict host keys, bounded concurrency, retry
//! backoff), Docker socket-proxy endpoints, and optional vendor APIs
//! (UniFi, Unraid, *arr media stack). Results are **redacted before
//! persistence** and written to the private cache under
//! `~/.cortex/inventory` (`normalized/homelab.json` + raw artifacts).
//!
//! Invariants: the MCP `map` action only reads the normalized cache — it
//! never triggers collection or returns raw artifact bodies. Refresh is
//! driven by the background task in `runtime/inventory_refresh.rs` (5 min
//! cadence + file watchers) or `cortex inventory refresh`.

pub mod adguard;
pub mod cache;
pub mod collectors;
pub mod config;
pub mod device;
pub mod docker;
pub mod http;
pub mod limits;
pub mod media_stack;
pub mod orchestrator;
pub mod process;
pub mod projects;
pub mod raw_configs;
pub mod redaction;
pub mod remote_configs;
pub mod remote_device;
pub mod remote_docker;
pub mod schema;
pub mod ssh;
pub mod storage;
pub mod tailscale;
pub mod unifi;
pub mod unraid;

pub use cache::{InventoryCacheStatus, inventory_status, is_not_found_error, read_inventory_cache};
pub use config::InventoryConfig;
pub use orchestrator::{
    InventoryRefreshOutcome, InventoryRefreshReport, refresh_inventory,
    refresh_inventory_with_inventory,
};
