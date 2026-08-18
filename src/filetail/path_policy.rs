use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::platform::metadata_identity;

pub(crate) fn validate_file_tail_path(path: &str) -> Result<()> {
    let raw = Path::new(path);
    if !raw.is_absolute() {
        bail!("file-tail path must be absolute");
    }
    let symlink_metadata = std::fs::symlink_metadata(raw)
        .map_err(|err| anyhow::anyhow!("file-tail path is not readable: {path}: {err}"))?;
    if symlink_metadata.file_type().is_symlink() {
        bail!("file-tail path must not be a symlink");
    }
    if !symlink_metadata.file_type().is_file() {
        bail!("file-tail path must be a regular file");
    }

    let canonical = std::fs::canonicalize(raw)
        .map_err(|err| anyhow::anyhow!("file-tail path could not be canonicalized: {err}"))?;
    let denied = [
        "/data",
        "/cortex-home",
        "/home/cortex/.ssh",
        "/home/cortex/workspace",
    ];
    if denied
        .iter()
        .any(|root| canonical.starts_with(Path::new(root)))
    {
        bail!("file-tail path is under a sensitive cortex mount");
    }

    let allowed_roots = canonical_allowed_file_tail_roots();
    if allowed_roots.iter().any(|root| canonical.starts_with(root)) {
        let file = std::fs::File::open(raw)
            .map_err(|err| anyhow::anyhow!("file-tail path is not readable: {path}: {err}"))?;
        drop(file);
        return Ok(());
    }

    bail!(
        "file-tail path is outside allowed roots: {}",
        canonical.display()
    );
}

pub(crate) fn validate_opened_file_tail_path(
    path: &str,
    opened_metadata: &std::fs::Metadata,
) -> Result<()> {
    if !opened_metadata.file_type().is_file() {
        bail!("file-tail opened path must be a regular file");
    }
    validate_file_tail_path(path)?;
    let path_metadata = std::fs::symlink_metadata(path)
        .map_err(|err| anyhow::anyhow!("file-tail path is not readable: {path}: {err}"))?;
    if metadata_identity(opened_metadata) != metadata_identity(&path_metadata) {
        bail!("file-tail path changed while opening");
    }
    Ok(())
}

fn canonical_allowed_file_tail_roots() -> Vec<PathBuf> {
    allowed_file_tail_roots()
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect()
}

#[cfg(not(test))]
fn allowed_file_tail_roots() -> Vec<PathBuf> {
    crate::env::var("CORTEX_FILE_TAIL_ALLOWED_ROOTS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|root| !root.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_else(|| {
            let roots = vec![PathBuf::from("/file-tail-root")];
            roots
        })
}

#[cfg(test)]
fn allowed_file_tail_roots() -> Vec<PathBuf> {
    let mut roots: Vec<_> = crate::env::var("CORTEX_FILE_TAIL_ALLOWED_ROOTS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|root| !root.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_else(|| vec![PathBuf::from("/file-tail-root")]);
    roots.push(std::env::temp_dir());
    roots
}
