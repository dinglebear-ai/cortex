//! Per-subcommand parser modules.
//!
//! Each module contains the `parse_*` functions for one CLI surface.
//! The parent `cli.rs` dispatches to these modules via the top-level
//! `parse_command` match arm.
//!
//! # Migration status (Q-C1)
//!
//! cli.rs was 5,005 LOC at the start of this work. Only `args.rs` had been
//! extracted. This change begins populating `src/cli/commands/` with leaf
//! subcommands. The hand-rolled `FlagCursor` parser and helpers in cli.rs
//! are shared via `pub(crate)` re-exports until a full clap-derive migration
//! is done.
//!
//! Extracted so far:
//! - `alerts` — canonical alerts/signatures/notifications command tree
//! - `clock_skew`, `anomalies`, `compare`, `apps` — surface
//!   parity gap-closure subcommands (2026-05-22).
//!
//! Remaining (each is ~50-100 LOC of parse functions):
//! db, setup, compose, config, hosts sources, timeline, patterns, etc.

pub(crate) mod alerts;
pub(crate) mod anomalies;
pub(crate) mod apps;
pub(crate) mod artifact_evidence;
pub(crate) mod clock_skew;
pub(crate) mod compare;
pub(crate) mod correlate_state;
pub(crate) mod file_tails;
pub(crate) mod fleet_state;
pub(crate) mod graph;
pub(crate) mod host_state;
pub(crate) mod ingest;
pub(crate) mod state;
pub(crate) mod topic_correlate;
