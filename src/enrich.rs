//! Enrichment framework — parser dispatch on the writer hot path.
//!
//! Spec: docs/history/superpowers/specs/2026-05/2026-05-16-enrichment-framework-design.md
//! Contract: docs/reference/contracts/parser-trait.rs
//!
//! Architecture:
//!   LogBatchEntry → AI scrub (existing) → dispatcher → parser → merge into entry
//!
//! Parser failure does NOT drop the row — `parse_error` records the diagnostic
//! and the row is written with whatever fields the parser populated before
//! failing.

pub mod dispatch;
pub mod output;
pub mod parser;
pub mod parsers;

pub use dispatch::EnrichmentPipeline;
pub use output::{merge_output, record_error, stamp_source_kind};
pub use parser::{
    AuthOutcome, Parser, ParserError, ParserId, ParserInput, ParserOutput, SourceKind,
};
