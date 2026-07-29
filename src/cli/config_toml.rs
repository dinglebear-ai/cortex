use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
// ---------------------------------------------------------------------------
// config.toml read/write (formatting-preserving via toml_edit)

pub(crate) fn load_toml_document(path: &std::path::Path) -> Result<toml_edit::DocumentMut> {
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => bail!("failed to read {}: {e}", path.display()),
    };
    contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| anyhow!("failed to parse {}: {e}", path.display()))
}

pub(crate) fn read_toml_value(path: &std::path::Path, key: &str) -> Result<Option<String>> {
    let segments = parse_toml_key(key)?;
    let doc = load_toml_document(path)?;
    let mut item: &toml_edit::Item = doc.as_item();
    for segment in &segments {
        match item.get(segment) {
            Some(next) => item = next,
            None => return Ok(None),
        }
    }
    Ok(Some(format_toml_item(item)))
}

pub(crate) fn write_toml_value(
    path: &std::path::Path,
    key: &str,
    raw_value: &str,
) -> Result<String> {
    let segments = parse_toml_key(key)?;
    let mut doc = load_toml_document(path)?;
    let value = parse_user_value(raw_value)?;

    {
        let (last, parents) = segments.split_last().expect("non-empty segments");
        let mut current: &mut toml_edit::Item = doc.as_item_mut();
        for segment in parents {
            match current.get(segment).map(classify_toml_item) {
                Some(TomlItemKind::Table) | Some(TomlItemKind::InlineTable) => {}
                Some(TomlItemKind::Other) => {
                    bail!(
                        "cannot set `{key}`: `{segment}` already exists as a non-table value \
                         (set or unset it directly first)"
                    );
                }
                None => {
                    if current.is_table() {
                        current
                            .as_table_mut()
                            .expect("checked")
                            .insert(segment, toml_edit::Item::Table(toml_edit::Table::new()));
                    } else {
                        bail!("cannot create `{key}`: parent is not a table");
                    }
                }
            }
            current = current
                .get_mut(segment)
                .ok_or_else(|| anyhow!("cannot descend into `{segment}`"))?;
        }
        if current.is_table() {
            current
                .as_table_mut()
                .expect("checked")
                .insert(last, toml_edit::Item::Value(value.clone()));
        } else if current.is_inline_table() {
            current
                .as_inline_table_mut()
                .expect("checked")
                .insert(last, value.clone());
        } else {
            bail!("cannot set `{key}`: parent is not a table");
        }
    }

    write_toml_file(path, &doc.to_string())?;
    Ok(format_value(&value))
}

pub(crate) fn remove_toml_value(path: &std::path::Path, key: &str) -> Result<Option<String>> {
    let segments = parse_toml_key(key)?;
    let mut doc = load_toml_document(path)?;

    let removed: Option<String> = {
        let (last, parents) = segments.split_last().expect("non-empty segments");
        let mut current: &mut toml_edit::Item = doc.as_item_mut();
        let mut missing = false;
        for segment in parents {
            let next_kind = current.get(segment).map(classify_toml_item);
            match next_kind {
                Some(TomlItemKind::Table) | Some(TomlItemKind::InlineTable) => {
                    current = current.get_mut(segment).expect("checked above");
                }
                Some(_) => bail!("cannot descend into `{segment}`: not a table"),
                None => {
                    missing = true;
                    break;
                }
            }
        }
        if missing {
            None
        } else if current.is_table() {
            current
                .as_table_mut()
                .expect("checked")
                .remove(last)
                .map(|item| format_toml_item(&item))
        } else if current.is_inline_table() {
            current
                .as_inline_table_mut()
                .expect("checked")
                .remove(last)
                .map(|val| format_value(&val))
        } else {
            bail!("cannot unset `{key}`: parent is not a table");
        }
    };

    if removed.is_some() {
        write_toml_file(path, &doc.to_string())?;
    }
    Ok(removed)
}

pub(crate) enum TomlItemKind {
    Table,
    InlineTable,
    Other,
}

pub(crate) fn classify_toml_item(item: &toml_edit::Item) -> TomlItemKind {
    if item.is_table() {
        TomlItemKind::Table
    } else if item.is_inline_table() {
        TomlItemKind::InlineTable
    } else {
        TomlItemKind::Other
    }
}

pub(crate) fn list_toml_entries(path: &std::path::Path) -> Result<Vec<(String, String)>> {
    let doc = load_toml_document(path)?;
    let mut out = Vec::new();
    flatten_toml(doc.as_item(), "", &mut out);
    Ok(out)
}

pub(crate) fn flatten_toml(item: &toml_edit::Item, prefix: &str, out: &mut Vec<(String, String)>) {
    match item {
        toml_edit::Item::Table(table) => {
            for (key, child) in table.iter() {
                let next = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_toml(child, &next, out);
            }
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table)) => {
            for (key, child) in table.iter() {
                let next = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_toml_value(child, &next, out);
            }
        }
        toml_edit::Item::Value(value) => {
            out.push((prefix.to_string(), format_value(value)));
        }
        toml_edit::Item::ArrayOfTables(_) | toml_edit::Item::None => {
            out.push((prefix.to_string(), format_toml_item(item)));
        }
    }
}

fn flatten_toml_value(value: &toml_edit::Value, prefix: &str, out: &mut Vec<(String, String)>) {
    if let toml_edit::Value::InlineTable(table) = value {
        for (key, child) in table.iter() {
            let next = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            flatten_toml_value(child, &next, out);
        }
    } else {
        out.push((prefix.to_string(), format_value(value)));
    }
}

pub(crate) fn parse_toml_key(key: &str) -> Result<Vec<String>> {
    if key.is_empty() {
        bail!("TOML key must not be empty");
    }
    let segments: Vec<String> = key.split('.').map(|s| s.to_string()).collect();
    for seg in &segments {
        if seg.is_empty() {
            bail!("TOML key segment must not be empty in `{key}`");
        }
    }
    Ok(segments)
}

pub(crate) fn parse_user_value(raw: &str) -> Result<toml_edit::Value> {
    if let Ok(item) = format!("__x = {raw}").parse::<toml_edit::DocumentMut>()
        && let Some(value) = item.get("__x").and_then(|i| i.as_value()).cloned()
    {
        return Ok(value);
    }
    let trimmed = raw.trim();
    // Bracket/brace shorthand must be valid TOML — refuse to silently coerce
    // `[a, b]` or `{ a = 1 }` to a string when the user clearly intended an
    // array or inline table.
    if let Some(first) = trimmed.chars().next()
        && matches!(first, '[' | '{')
    {
        bail!(
            "value `{raw}` looks like a TOML array/inline-table but failed to parse; \
                 quote each element (e.g. `[\"a\", \"b\"]`) or pass it as a quoted string"
        );
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "true" => return Ok(toml_edit::Value::from(true)),
        "false" => return Ok(toml_edit::Value::from(false)),
        _ => {}
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Ok(toml_edit::Value::from(n));
    }
    if let Ok(n) = trimmed.parse::<f64>()
        && n.is_finite()
    {
        return Ok(toml_edit::Value::from(n));
    }
    Ok(toml_edit::Value::from(raw))
}

pub(crate) fn format_toml_item(item: &toml_edit::Item) -> String {
    match item {
        toml_edit::Item::Value(v) => format_value(v),
        toml_edit::Item::Table(_) | toml_edit::Item::ArrayOfTables(_) => {
            item.to_string().trim().to_string()
        }
        toml_edit::Item::None => String::new(),
    }
}

pub(crate) fn format_value(value: &toml_edit::Value) -> String {
    let mut cloned = value.clone();
    let decor = cloned.decor_mut();
    decor.set_prefix("");
    decor.set_suffix("");
    cloned.to_string()
}

pub(crate) fn write_toml_file(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    let temp_path = atomic_write_path(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let mode = std::fs::metadata(path)
            .map(|metadata| metadata.mode() & 0o777)
            .unwrap_or(0o644);
        options.mode(mode);
    }
    let write_result = (|| -> Result<()> {
        let mut file = options
            .open(&temp_path)
            .map_err(|e| anyhow!("failed to open {}: {e}", temp_path.display()))?;
        file.write_all(contents.as_bytes())
            .map_err(|e| anyhow!("failed to write {}: {e}", temp_path.display()))?;
        file.sync_all()
            .map_err(|e| anyhow!("failed to sync {}: {e}", temp_path.display()))?;
        std::fs::rename(&temp_path, path).map_err(|e| {
            anyhow!(
                "failed to replace {} with {}: {e}",
                path.display(),
                temp_path.display()
            )
        })?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

fn atomic_write_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config.toml");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{file_name}.tmp.{}.{}", std::process::id(), count))
}

#[cfg(test)]
#[path = "config_toml_tests.rs"]
mod tests;
