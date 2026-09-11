//! `cortex reflect`: one-shot local skill / MCP / hook reflection report.

use std::path::PathBuf;

use cortex::app::ReflectKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReflectArgs {
    /// Normalized absolute timestamp (default: 7 days ago).
    pub since: String,
    pub until: Option<String>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub kinds: Vec<ReflectKind>,
    pub no_llm: bool,
    pub max_assess: u32,
    pub no_index: bool,
    pub db: Option<PathBuf>,
    pub json: bool,
}
