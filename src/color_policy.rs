//! Color policy — the single source of truth for "should we emit color?"
//!
//! Rendering palettes are intentionally split by surface:
//! - `crate::logging::aurora` — ANSI 256 for the tracing console + diagnostics
//! - the binary's `cli::color` — 24-bit truecolor for CLI data output
//!
//! But the *decision* (the `--color` override plus `NO_COLOR` /
//! `FORCE_COLOR` / TTY detection) must be unified, because the dependency
//! graph only runs binary → lib. The binary sets the override here; both the
//! binary's `cli::color` and the lib's `logging` / `doctor` / `setup` surfaces
//! read it via [`resolve`]. A single `--color=never` then mutes every surface,
//! including tracing.

use crate::env;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicU8, Ordering};

const COLOR_AUTO: u8 = 0;
const COLOR_ALWAYS: u8 = 1;
const COLOR_NEVER: u8 = 2;

/// Runtime color choice. Set once at startup before any output is produced.
///
/// Ordering: `install_color_choice` is called from the main thread before the
/// tokio worker spawn; readers observe via the happens-before edge. Relaxed is
/// safe under that single-writer-before-readers contract.
static COLOR_OVERRIDE: AtomicU8 = AtomicU8::new(COLOR_AUTO);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

/// Install the runtime color choice (called once at startup, by the binary).
pub fn install_color_choice(choice: ColorChoice) {
    let val = match choice {
        ColorChoice::Auto => COLOR_AUTO,
        ColorChoice::Always => COLOR_ALWAYS,
        ColorChoice::Never => COLOR_NEVER,
    };
    COLOR_OVERRIDE.store(val, Ordering::Relaxed);
}

/// Whether color should be emitted to a stream with the given TTY status.
///
/// Precedence: explicit `--color` override → `NO_COLOR` → `FORCE_COLOR` /
/// `CLICOLOR_FORCE` → the stream's TTY status.
pub fn resolve(is_terminal: bool) -> bool {
    match COLOR_OVERRIDE.load(Ordering::Relaxed) {
        COLOR_ALWAYS => true,
        COLOR_NEVER => false,
        _ => {
            if env::var_os("NO_COLOR").is_some() {
                return false;
            }
            if env_flag("FORCE_COLOR") || env_flag("CLICOLOR_FORCE") {
                return true;
            }
            is_terminal
        }
    }
}

/// Convenience: resolve against stdout's TTY status.
pub fn enabled_stdout() -> bool {
    resolve(std::io::stdout().is_terminal())
}

/// Convenience: resolve against stderr's TTY status.
pub fn enabled_stderr() -> bool {
    resolve(std::io::stderr().is_terminal())
}

fn env_flag(var: &str) -> bool {
    env::var_os(var)
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "color_policy_tests.rs"]
mod tests;
