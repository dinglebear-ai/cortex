//! Persistent host identity generation and validation.
use std::path::Path;

use anyhow::{Context, Result, bail};
use getrandom::fill as random_fill;

pub fn load_or_create_host_id(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(existing) => {
            let host_id = existing.trim();
            validate_host_id(host_id)?;
            Ok(host_id.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let host_id = generate_host_id()?;
            write_private(path, &format!("{host_id}\n"))?;
            Ok(host_id)
        }
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

pub(super) fn validate_host_id(host_id: &str) -> Result<()> {
    if host_id.len() < 16
        || !host_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("invalid heartbeat host_id in config");
    }
    Ok(())
}

fn generate_host_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    random_fill(&mut bytes).context("generate heartbeat host id")?;
    Ok(format!("syslog_{}", hex_bytes(&bytes)))
}

pub(super) fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn write_private(path: &Path, content: &str) -> Result<()> {
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
