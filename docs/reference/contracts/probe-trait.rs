//! Probe trait contract — **source of truth** for the probe-registry interface
//! (epic `cortex-fue9`, depends on agent-mode `cortex-qgnx`).
//!
//! This file lives at `docs/reference/contracts/probe-trait.rs` and is **the contract**.
//! Implementations in the per-host agent binary (`src/agent/probes/*`) and
//! the dispatcher (`src/agent/registry.rs`) MUST conform to these types.
//!
//! ## Compilation
//!
//! This file is intended to compile cleanly against a crate with the
//! following dependencies:
//!
//! ```toml
//! serde = { version = "1", features = ["derive"] }
//! serde_json = "1"
//! thiserror = "1"
//! tokio = { version = "1", features = ["time"] }
//! ```
//!
//! `thiserror` is not yet in the cortex `Cargo.toml`; add it before
//! dropping the real implementations. `async_trait` is intentionally NOT
//! used — the trait below uses the `impl Future` return form available on
//! the project's MSRV (1.86, Cargo.toml). Implementers may switch to
//! `#[async_trait]` if dynamic dispatch is required; in that case the
//! signature becomes `async fn run(...)` and the trait must be
//! `?Send`-aware.
//!
//! ## Where this file lives in the crate
//!
//! When implementers begin coding, copy the types into
//! `src/agent/probe.rs` and keep this file as the human-readable reference.

use std::future::Future;
use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// ProbeInput — args the agent receives over the WebSocket / scheduler
// ---------------------------------------------------------------------------

/// What every probe receives. `args` is the typed payload specific to the
/// probe (e.g. [`MemTopArgs`], [`DiskBlackholesArgs`]). The remaining fields
/// are common across every probe call site (scheduled, on-demand RPC, or
/// local fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeInput {
    /// Probe-specific arguments, deserialised by the probe from JSON.
    /// Allowed to be `Value::Null` when the probe takes no parameters.
    pub args: serde_json::Value,

    /// Host running the agent. Used by the agent for log enrichment and by
    /// the server for `probe_results.host_id`.
    pub host_id: String,

    /// Who triggered this run. `"schedule"` for cadence-driven runs,
    /// `"mcp:<client>"` for MCP-action-driven runs, `"rpc"` for direct
    /// JSON-RPC.
    pub requested_by: String,

    /// Correlation id from the JSON-RPC request, `None` for scheduled runs.
    pub request_id: Option<String>,

    /// Hard deadline for the run. Probes MUST respect this — exceeding it
    /// produces a [`ProbeError::Timeout`].
    pub timeout_ms: u32,
}

// ---------------------------------------------------------------------------
// Per-probe payload structs
// ---------------------------------------------------------------------------

// --- disk.usage -----------------------------------------------------------

/// One mount entry in the [`ProbeOutput::DiskUsage`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountInfo {
    pub mountpoint: String,
    pub fs_type: String,
    pub device: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub avail_bytes: u64,
    pub used_pct: f32,
    pub inodes_total: Option<u64>,
    pub inodes_used: Option<u64>,
}

// --- disk.blackholes ------------------------------------------------------

/// One walked directory in the [`ProbeOutput::DiskBlackholes`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackholeEntry {
    pub path: String,
    pub total_bytes: u64,
    pub file_count: u64,
    pub dir_count: u64,
    /// Unix epoch seconds.
    pub newest_mtime: Option<i64>,
    /// Unix epoch seconds.
    pub oldest_mtime: Option<i64>,
    /// `false` when the walker hit its time or entry budget.
    pub completed: bool,
}

// --- mem.top --------------------------------------------------------------

/// One process in the [`ProbeOutput::MemTop`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: i32,
    pub ppid: i32,
    pub comm: String,
    /// First 256 chars of `/proc/<pid>/cmdline`.
    pub cmdline: String,
    pub uid: u32,
    pub rss_bytes: u64,
    pub vsize_bytes: u64,
    /// First cgroup v2 path, if available.
    pub cgroup: Option<String>,
}

// --- net.neigh ------------------------------------------------------------

/// One ARP / NDP neighbour entry in the [`ProbeOutput::NetNeigh`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighEntry {
    pub ip: IpAddr,
    pub mac: Option<String>,
    pub dev: String,
    /// One of `REACHABLE`, `STALE`, `DELAY`, `FAILED`, `PERMANENT`, `NOARP`.
    pub state: String,
    pub age_secs: Option<u32>,
}

// --- net.dns_check --------------------------------------------------------

/// One DNS probe result in the [`ProbeOutput::NetDnsCheck`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsCheckResult {
    pub hostname: String,
    pub resolver: IpAddr,
    /// One of `ok`, `timeout`, `nxdomain`, `servfail`, `error`.
    pub status: String,
    pub latency_ms: Option<u32>,
    pub answers: Vec<IpAddr>,
    pub error: Option<String>,
}

// --- systemd.failed -------------------------------------------------------

/// One systemd unit in the [`ProbeOutput::SystemdFailed`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedUnit {
    pub name: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub description: String,
    pub n_restarts: Option<u32>,
}

// --- docker.health --------------------------------------------------------

/// One container in the [`ProbeOutput::DockerHealth`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerHealth {
    /// Short id (12 hex chars).
    pub id: String,
    pub name: String,
    pub image: String,
    /// One of `running`, `exited`, `restarting`, `paused`, `dead`.
    pub state: String,
    /// `healthy` | `unhealthy` | `starting` | `none`.
    pub health: Option<String>,
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
    /// RFC3339 timestamp.
    pub started_at: Option<String>,
    pub uptime_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// ProbeOutput — tagged union, one variant per probe
// ---------------------------------------------------------------------------

/// The output of any probe. Serialised as a tagged enum with `probe` as the
/// discriminator and `payload` as the body, matching the spec wire format:
///
/// ```json
/// { "probe": "mem.top", "payload": { "processes": [...] } }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "probe", content = "payload")]
pub enum ProbeOutput {
    /// `disk.usage` — per-mountpoint capacity.
    #[serde(rename = "disk.usage")]
    DiskUsage { mounts: Vec<MountInfo> },

    /// `disk.blackholes` — high-churn build-cache paths.
    #[serde(rename = "disk.blackholes")]
    DiskBlackholes {
        entries: Vec<BlackholeEntry>,
        /// Paths whose walker hit its time or entry budget.
        truncated_paths: Vec<String>,
    },

    /// `mem.top` — top-N processes by RSS.
    #[serde(rename = "mem.top")]
    MemTop {
        total_mem_bytes: u64,
        avail_mem_bytes: u64,
        processes: Vec<ProcessInfo>,
    },

    /// `mem.pressure` — PSI memory pressure (some + full buckets, three
    /// averaging windows each plus total_us).
    #[serde(rename = "mem.pressure")]
    MemPressure {
        avg10: f32,
        avg60: f32,
        avg300: f32,
        full_avg10: f32,
        full_avg60: f32,
        full_avg300: f32,
        some_total_us: u64,
        full_total_us: u64,
    },

    /// `net.neigh` — ARP/NDP neighbour table.
    #[serde(rename = "net.neigh")]
    NetNeigh { entries: Vec<NeighEntry> },

    /// `net.dns_check` — per-(hostname × resolver) probe.
    #[serde(rename = "net.dns_check")]
    NetDnsCheck { results: Vec<DnsCheckResult> },

    /// `systemd.failed` — failed unit enumeration.
    #[serde(rename = "systemd.failed")]
    SystemdFailed {
        /// `running` | `degraded` | `maintenance` | ...
        system_state: String,
        units: Vec<FailedUnit>,
    },

    /// `docker.health` — per-container state.
    #[serde(rename = "docker.health")]
    DockerHealth {
        containers: Vec<ContainerHealth>,
        daemon_version: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// ProbeError — failure variants
// ---------------------------------------------------------------------------

/// Failure modes a probe can produce. Recorded in `probe_results.status`
/// (column values `"timeout"`, `"unsupported"`, `"error"`) plus the
/// `Display` form in `probe_results.error`.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// The probe exceeded its deadline.
    #[error("probe timeout after {0:?}")]
    Timeout(Duration),

    /// The probe is not supported on this host's OS / kernel.
    #[error("not supported on this OS")]
    Unsupported,

    /// A required capability is absent (e.g. `docker.sock`,
    /// `/proc/pressure/memory`, the systemd D-Bus interface).
    #[error("missing capability: {0}")]
    MissingCapability(String),

    /// The probe's typed `args` failed to deserialise from the JSON
    /// `ProbeInput.args` value, or a value was out of range.
    #[error("invalid args: {0}")]
    InvalidArgs(String),

    /// Underlying I/O failure (file read, socket, netlink).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Catch-all for unexpected failures inside the probe.
    #[error("internal: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Probe trait
// ---------------------------------------------------------------------------

/// One concrete probe implementation. Stateless — all state lives in
/// [`ProbeInput`]. Probes are dispatched via a `&'static dyn Probe` from
/// the registry; their lifetime is the agent process lifetime.
///
/// Implementers MUST:
///  * respect `input.timeout_ms` (probes are wrapped in
///    `tokio::time::timeout` by the runner, but probes that hold blocking
///    file walks need explicit checks);
///  * be cancel-safe (futures dropped mid-run leave no resources behind);
///  * keep `ProbeOutput` JSON-serialised under 1 MiB (the agent rejects
///    larger payloads before sending).
pub trait Probe: Send + Sync + 'static {
    /// Stable name: e.g. `"disk.blackholes"`. Used in registry lookup, in
    /// the JSON-RPC `params.probe` field, and in the `probe_results.probe_name`
    /// column.
    fn name(&self) -> &'static str;

    /// Default schedule cadence. `None` = on-demand only (probe is never
    /// dispatched by the scheduler; only direct `probe.run` RPCs trigger
    /// it). The server may override this via `schedule.set`.
    fn schedule_default() -> Option<Duration>
    where
        Self: Sized;

    /// Execute the probe. The future MUST resolve within
    /// `input.timeout_ms` or the runner will drop it and emit
    /// [`ProbeError::Timeout`].
    ///
    /// Note on dyn-compatibility: returning `impl Future` from a trait
    /// method (RPITIT) is stable on the project MSRV but is not
    /// dyn-compatible. The registry stores probes as a `&'static dyn
    /// Probe`, so the **real** implementation in `src/agent/probe.rs`
    /// must wrap this with `#[async_trait::async_trait]` (returning
    /// `Pin<Box<dyn Future<Output=…> + Send + '_>>`). This contract uses
    /// the simpler RPITIT form to stay free of the `async_trait`
    /// dependency; switching to `async_trait` is a mechanical edit when
    /// dropping the file into the crate.
    fn run(&self, input: ProbeInput) -> impl Future<Output = Result<ProbeOutput, ProbeError>>;
}

// ---------------------------------------------------------------------------
// PROBE_NAMES — closed registry of V1 probe names
// ---------------------------------------------------------------------------

/// Closed registry of probe names. The capability handshake (`agent.hello.capabilities.supported_probes`)
/// MUST advertise a subset of these strings. Server-side rejects values not in this set.
///
/// Wire format is dotted, lowercase, dot-separated category (e.g. `disk.usage` not `disk_usage`).
/// Adding a name is backwards-compatible. Renaming is a major version bump.
pub const PROBE_NAMES: &[&str] = &[
    "disk.usage",
    "disk.blackholes",
    "mem.top",
    "mem.pressure",
    "net.neigh",
    "net.dns_check",
    "systemd.failed",
    "docker.health",
];

/// Validates that a probe name string is a recognised V1 probe.
pub fn is_known_probe(name: &str) -> bool {
    PROBE_NAMES.contains(&name)
}
