//! `cortex reflect` dispatch, database path resolution, and database file
//! permissions.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cortex::app::ReflectRequest;
use cortex::app::reflect_report::render_reflect_markdown;

use super::CliMode;
use super::args::ReflectArgs;

pub(crate) async fn run_reflect(mode: &CliMode, args: ReflectArgs) -> Result<()> {
    // main.rs rejects HTTP mode before building a runtime; this guard is
    // defensive for any other caller.
    let CliMode::Local(service) = mode else {
        bail!(
            "cortex reflect runs locally; remove --http / --server / --token and unset CORTEX_USE_HTTP"
        );
    };
    let request = ReflectRequest {
        since: Some(args.since.clone()),
        until: args.until.clone(),
        project: args.project.clone(),
        tool: args.tool.clone(),
        kinds: args.kinds.clone(),
        run_llm: !args.no_llm,
        max_assess: args.max_assess,
        index: !args.no_index,
    };
    let report = service
        .run_reflect(request, |line| eprintln!("[reflect] {line}"))
        .await?;
    if let Some(reason) = &report.llm_fallback_reason {
        eprintln!("[reflect] warning: {reason}");
    }
    let body = if args.json {
        let mut json = serde_json::to_string_pretty(&report)?;
        json.push('\n');
        json
    } else {
        render_reflect_markdown(&report)
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(body.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReflectDbSource {
    Flag,
    Env,
    Default,
}

/// Reflect database: `--db`, then non-empty `CORTEX_DB_PATH`, then
/// `~/.cortex/reflect.db`. Environment values are passed in so the order is
/// unit-testable.
pub(crate) fn resolve_reflect_db_path(
    flag: Option<&Path>,
    env_db_path: Option<OsString>,
    home: Option<OsString>,
) -> Result<(PathBuf, ReflectDbSource)> {
    if let Some(path) = flag {
        return Ok((path.to_path_buf(), ReflectDbSource::Flag));
    }
    if let Some(path) = env_db_path.filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(path), ReflectDbSource::Env));
    }
    let home = home.filter(|value| !value.is_empty()).ok_or_else(|| {
        anyhow!("cannot resolve ~/.cortex/reflect.db because HOME is not set; pass --db PATH")
    })?;
    Ok((
        PathBuf::from(home).join(".cortex").join("reflect.db"),
        ReflectDbSource::Default,
    ))
}

/// Creates the default database directory owner-only when it does not
/// exist. Existing directories and non-default paths are left alone.
pub(crate) fn prepare_reflect_db_dir(path: &Path, source: ReflectDbSource) -> Result<()> {
    if source != ReflectDbSource::Default {
        return Ok(());
    }
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting {}", parent.display()))?;
    }
    Ok(())
}

/// Restricts the SQLite file and its `-wal`/`-shm` siblings to the owner.
pub(crate) fn restrict_reflect_db_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let file = PathBuf::from(name);
            if file.is_file() {
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("restricting {}", file.display()))?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[path = "dispatch_reflect_tests.rs"]
mod tests;
