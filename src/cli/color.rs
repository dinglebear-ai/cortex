use anyhow::{Result, bail};
use cortex::color_policy::{self, ColorChoice};

// Aurora design-system CLI tokens (dark-first, operator-grade palette).
// Keep in sync with aurora-design-system/registry/aurora/styles/aurora.css
pub(crate) const PRIMARY_ANSI: &str = "\x1b[38;2;230;244;251m"; // #e6f4fb
pub(crate) const MUTED_ANSI: &str = "\x1b[38;2;167;188;201m"; //   #a7bcc9
pub(crate) const CYAN_ANSI: &str = "\x1b[38;2;41;182;246m"; //    #29b6f6
pub(crate) const SUCCESS_ANSI: &str = "\x1b[38;2;125;211;199m"; // #7dd3c7
pub(crate) const WARN_ANSI: &str = "\x1b[38;2;198;163;107m"; //   #c6a36b
pub(crate) const ERROR_ANSI: &str = "\x1b[38;2;199;132;144m"; //  #c78490
pub(crate) const VIOLET_ANSI: &str = "\x1b[38;2;167;139;250m"; // #a78bfa

/// Whether color should be emitted to stdout. Delegates to the unified policy
/// in [`cortex::color_policy`] (runtime override + NO_COLOR / FORCE_COLOR /
/// CLICOLOR_FORCE + TTY detection).
pub(crate) fn color_enabled() -> bool {
    color_policy::enabled_stdout()
}

/// Whether color should be emitted to stderr — used for the help banner and
/// top-level error reporting, which print to stderr (so `cortex --help | less`
/// still colors when stderr is a TTY).
pub(crate) fn color_enabled_stderr() -> bool {
    color_policy::enabled_stderr()
}

/// Parse and strip `--color` / `--no-color` from `args`, installing the runtime
/// color choice. Called once at startup, before `Mode::parse`, so every surface
/// (help, version, query, doctor, setup, tracing) honors the same switch.
///
/// Accepts: `--no-color`, `--color` (bare ⇒ always), `--color=VALUE`,
/// `--color VALUE` where VALUE ∈ `always|never|auto`. Stops at a `--` sentinel
/// so wrapped commands (`cortex ingest shell agent wrap -- cmd --color`) are left
/// untouched, mirroring [`super::run::GlobalFlags::extract`].
pub(crate) fn install_color_from_args(args: &mut Vec<String>) -> Result<()> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            break;
        }
        if arg == "--no-color" {
            color_policy::install_color_choice(ColorChoice::Never);
            args.remove(i);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--color=") {
            color_policy::install_color_choice(parse_color_value(value)?);
            args.remove(i);
            continue;
        }
        if arg == "--color" {
            // `--color VALUE` if a recognized value follows, else bare `--color` ⇒ always.
            if let Some(next) = args.get(i + 1).map(String::as_str)
                && matches!(next, "always" | "never" | "auto")
            {
                color_policy::install_color_choice(parse_color_value(next)?);
                args.remove(i + 1);
                args.remove(i);
                continue;
            }
            color_policy::install_color_choice(ColorChoice::Always);
            args.remove(i);
            continue;
        }
        i += 1;
    }
    Ok(())
}

fn parse_color_value(value: &str) -> Result<ColorChoice> {
    match value {
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        "auto" => Ok(ColorChoice::Auto),
        other => bail!("--color expects always|never|auto, got `{other}`"),
    }
}

pub(crate) fn ansi_colorize(code: &str, text: &str) -> String {
    if color_enabled() {
        format!("{code}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Like [`ansi_colorize`] but gates on stderr's TTY status — for output written
/// to stderr (the help banner, top-level error reporting).
pub(crate) fn ansi_colorize_stderr(code: &str, text: &str) -> String {
    if color_enabled_stderr() {
        format!("{code}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

// ── Free color helpers — use these instead of Palette ──────────────────────

pub(crate) fn primary(text: &str) -> String {
    ansi_colorize(PRIMARY_ANSI, text)
}

pub(crate) fn muted(text: &str) -> String {
    ansi_colorize(MUTED_ANSI, text)
}

pub(crate) fn cyan(text: &str) -> String {
    ansi_colorize(CYAN_ANSI, text)
}

pub(crate) fn success(text: &str) -> String {
    ansi_colorize(SUCCESS_ANSI, text)
}

pub(crate) fn warn(text: &str) -> String {
    ansi_colorize(WARN_ANSI, text)
}

pub(crate) fn error(text: &str) -> String {
    ansi_colorize(ERROR_ANSI, text)
}

pub(crate) fn violet(text: &str) -> String {
    ansi_colorize(VIOLET_ANSI, text)
}

pub(crate) fn severity(sev: &str) -> String {
    if !color_enabled() {
        return sev.to_string();
    }
    let lower = sev.to_ascii_lowercase();
    if lower.starts_with("err") || lower == "crit" || lower == "alert" || lower == "emerg" {
        ansi_colorize(ERROR_ANSI, sev)
    } else if lower.starts_with("warn") {
        ansi_colorize(WARN_ANSI, sev)
    } else if lower == "info" || lower == "notice" {
        ansi_colorize(SUCCESS_ANSI, sev)
    } else {
        ansi_colorize(MUTED_ANSI, sev)
    }
}

/// `"42 logs"` — both parts colored cyan.
#[allow(dead_code)]
pub(crate) fn metric(value: impl std::fmt::Display, label: &str) -> String {
    format!("{} {}", cyan(&value.to_string()), cyan(label))
}

/// `"error: <msg>"` on stderr in Aurora rose-red. Gates on stderr's TTY.
pub(crate) fn report_error(msg: &str) {
    eprintln!("{} {}", ansi_colorize_stderr(ERROR_ANSI, "error:"), msg);
}

/// `"hint: <msg>"` on stderr in Aurora cyan — companion to report_error.
#[allow(dead_code)]
pub(crate) fn report_hint(msg: &str) {
    eprintln!("{} {}", ansi_colorize_stderr(CYAN_ANSI, "hint:"), msg);
}

#[allow(dead_code)]
pub(crate) fn symbol_for_status(status: &str) -> String {
    match status {
        "ok" | "completed" => success("✓"),
        "failed" | "error" => error("✗"),
        "running" | "processing" => cyan("◐"),
        "warn" | "warning" => warn("⚠"),
        _ => cyan("•"),
    }
}

#[allow(dead_code)]
pub(crate) fn status_text(status: &str) -> String {
    match status {
        "ok" | "completed" => success(status),
        "failed" | "error" => error(status),
        "running" | "processing" => cyan(status),
        "warn" | "warning" => warn(status),
        _ => cyan(status),
    }
}
