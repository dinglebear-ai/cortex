//! Merge selected MCP clients without replacing unrelated settings.
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
};
#[cfg(unix)]
fn validate_owner(uid: u32) -> io::Result<()> {
    if uid != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "client config must be owned by this user",
        ));
    }
    Ok(())
}

fn validate_config_file(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "client config must be a regular non-symlink file",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                validate_owner(metadata.uid())?;
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn atomic_private_write(path: &Path, content: &[u8], _mode: u32) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing config parent"))?;
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
    }
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::other(
            "config parent must be a non-symlink directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::other("config parent must be owned by this user"));
        }
    }
    validate_config_file(path)?;
    let temp = path.with_extension(format!(
        "cortex-tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&temp)?;
        file.write_all(content)?;
        file.sync_all()?;
        validate_config_file(path)?;
        std::fs::rename(&temp, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

pub fn client_paths(home: &Path, clients: &[String]) -> io::Result<Vec<PathBuf>> {
    clients
        .iter()
        .map(|c| match c.as_str() {
            "codex" => Ok(crate::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("config.toml")),
            "claude" => Ok(home.join(".claude.json")),
            "gemini" => Ok(home.join(".gemini/settings.json")),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown MCP client: {c}"),
            )),
        })
        .collect()
}

fn read_config(path: &Path) -> io::Result<String> {
    validate_config_file(path)?;
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(raw),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

fn render(client: &str, raw: &str, url: &str, token: Option<&str>) -> io::Result<String> {
    let invalid = |e: String| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {client} configuration: {e}"),
        )
    };
    if client == "codex" {
        let mut doc = raw.parse::<toml_edit::DocumentMut>().map_err(|error| {
            // TOML Display includes source snippets, which may contain credentials.
            let location = error
                .span()
                .and_then(|span| raw.get(..span.start))
                .map(|prefix| {
                    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
                    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
                    format!(" at line {line}, column {column}")
                })
                .unwrap_or_default();
            invalid(format!("TOML syntax error{location}"))
        })?;
        if doc.get("mcp_servers").is_some_and(|v| !v.is_table_like()) {
            return Err(invalid("mcp_servers must be a table".into()));
        }
        if doc.get("mcp_servers").is_none() {
            doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        if doc["mcp_servers"]
            .get("cortex")
            .is_some_and(|v| !v.is_table_like())
        {
            return Err(invalid("cortex must be a table".into()));
        }
        if doc["mcp_servers"].get("cortex").is_none() {
            doc["mcp_servers"]["cortex"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let table = doc["mcp_servers"]["cortex"].as_table_like_mut().unwrap();
        for key in ["command", "args", "env", "bearer_token_env_var"] {
            table.remove(key);
        }
        table.insert("url", toml_edit::value(url));
        for key in ["http_headers", "env_http_headers"] {
            if let Some(headers) = table.get_mut(key) {
                let headers = headers
                    .as_table_like_mut()
                    .ok_or_else(|| invalid(format!("{key} must be a table")))?;
                let authorization_keys = headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case("Authorization"))
                    .map(|(name, _)| name.to_owned())
                    .collect::<Vec<_>>();
                for name in authorization_keys {
                    headers.remove(&name);
                }
            }
        }
        if let Some(token) = token {
            if table.get("http_headers").is_none() {
                table.insert(
                    "http_headers",
                    toml_edit::Item::Value(toml_edit::Value::InlineTable(
                        toml_edit::InlineTable::new(),
                    )),
                );
            }
            table
                .get_mut("http_headers")
                .unwrap()
                .as_table_like_mut()
                .unwrap()
                .insert("Authorization", toml_edit::value(format!("Bearer {token}")));
        }
        Ok(doc.to_string())
    } else {
        let mut doc: Value = if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw).map_err(|e| invalid(e.to_string()))?
        };
        let obj = doc
            .as_object_mut()
            .ok_or_else(|| invalid("root must be an object".into()))?;
        let servers = obj
            .entry("mcpServers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| invalid("mcpServers must be an object".into()))?;
        let entry = servers
            .entry("cortex")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| invalid("cortex must be an object".into()))?;
        for key in ["command", "args", "env", "url", "httpUrl", "type"] {
            entry.remove(key);
        }
        if client == "claude" {
            entry.insert("type".into(), json!("http"));
            entry.insert("url".into(), json!(url));
        } else {
            entry.insert("httpUrl".into(), json!(url));
        }
        if entry.contains_key("headers") || token.is_some() {
            let headers = entry
                .entry("headers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| invalid("headers must be an object".into()))?;
            headers.retain(|name, _| !name.eq_ignore_ascii_case("Authorization"));
            if let Some(token) = token {
                headers.insert("Authorization".into(), json!(format!("Bearer {token}")));
            }
            headers.sort_keys();
        }
        entry.sort_keys();
        Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&doc).map_err(|e| invalid(e.to_string()))?
        ))
    }
}

/// Parse and prepare every client before the first write. Originals are private backups.
pub fn configure_clients(
    home: &Path,
    clients: &[String],
    base: &str,
    token: Option<&str>,
    dry_run: bool,
) -> io::Result<Vec<PathBuf>> {
    let url = format!(
        "{}/mcp",
        super::verification::normalize_server_url(base).map_err(io::Error::other)?
    );
    let paths = client_paths(home, clients)?;
    let plans = clients
        .iter()
        .zip(&paths)
        .map(|(c, p)| {
            let raw = read_config(p)?;
            let out = render(c, &raw, &url, token)?;
            Ok((p, raw, out))
        })
        .collect::<io::Result<Vec<_>>>()?;
    if !dry_run {
        for (path, raw, out) in plans {
            if raw == out {
                continue;
            }
            if read_config(path)? != raw {
                return Err(io::Error::other(
                    "client configuration changed concurrently; rerun setup",
                ));
            }
            if !raw.is_empty() {
                let backup = path.with_extension(format!(
                    "cortex-backup-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                atomic_private_write(&backup, raw.as_bytes(), 0o600)?;
            }
            atomic_private_write(path, out.as_bytes(), 0o600)?;
        }
    }
    Ok(paths)
}

#[cfg(test)]
#[path = "client_config_tests.rs"]
mod tests;
