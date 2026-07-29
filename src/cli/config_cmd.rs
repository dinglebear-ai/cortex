use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::config_toml::{list_toml_entries, read_toml_value, remove_toml_value, write_toml_value};
use super::output::common::print_json;
use super::{
    ConfigCommand, ConfigGetArgs, ConfigListArgs, ConfigSetArgs, ConfigTarget, ConfigUnsetArgs,
};
pub(crate) fn run_config(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Get(args) => run_config_get(args),
        ConfigCommand::Set(args) => run_config_set(args),
        ConfigCommand::Unset(args) => run_config_unset(args),
        ConfigCommand::List(args) => run_config_list(args),
    }
}

fn run_config_get(args: ConfigGetArgs) -> Result<()> {
    let target = resolve_target(&args.key, args.target)?;
    match target {
        ConfigTarget::Env => {
            let path = env_file_path()?;
            let value = read_env_kv(&path, &args.key)?;
            print_config_value(&args.key, value.as_deref(), "env", &path, args.json)
        }
        ConfigTarget::Toml => {
            let path = toml_file_path(args.toml_path.as_deref());
            let value = read_toml_value(&path, &args.key)?;
            print_config_value(&args.key, value.as_deref(), "toml", &path, args.json)
        }
        ConfigTarget::Auto => unreachable!("resolve_target never returns Auto"),
    }
}

fn run_config_set(args: ConfigSetArgs) -> Result<()> {
    let target = resolve_target(&args.key, args.target)?;
    match target {
        ConfigTarget::Env => {
            validate_env_key(&args.key)?;
            let path = env_file_path()?;
            let previous = read_env_kv(&path, &args.key)?;
            write_env_value(&path, &args.key, &args.value)?;
            print_config_set(
                &args.key,
                previous.as_deref(),
                &args.value,
                "env",
                &path,
                args.json,
            )
        }
        ConfigTarget::Toml => {
            let path = toml_file_path(args.toml_path.as_deref());
            let previous = read_toml_value(&path, &args.key)?;
            let stored = write_toml_value(&path, &args.key, &args.value)?;
            print_config_set(
                &args.key,
                previous.as_deref(),
                &stored,
                "toml",
                &path,
                args.json,
            )
        }
        ConfigTarget::Auto => unreachable!("resolve_target never returns Auto"),
    }
}

fn run_config_unset(args: ConfigUnsetArgs) -> Result<()> {
    let target = resolve_target(&args.key, args.target)?;
    match target {
        ConfigTarget::Env => {
            let path = env_file_path()?;
            let removed = remove_env_value(&path, &args.key)?;
            print_config_unset(&args.key, removed.as_deref(), "env", &path, args.json)
        }
        ConfigTarget::Toml => {
            let path = toml_file_path(args.toml_path.as_deref());
            let removed = remove_toml_value(&path, &args.key)?;
            print_config_unset(&args.key, removed.as_deref(), "toml", &path, args.json)
        }
        ConfigTarget::Auto => unreachable!("resolve_target never returns Auto"),
    }
}

fn run_config_list(args: ConfigListArgs) -> Result<()> {
    let mut env_entries: Option<(PathBuf, Vec<(String, String)>)> = None;
    let mut toml_entries: Option<(PathBuf, Vec<(String, String)>)> = None;

    if matches!(args.target, ConfigTarget::Auto | ConfigTarget::Env) {
        let path = env_file_path()?;
        let entries = list_env_entries(&path)?;
        env_entries = Some((path, entries));
    }
    if matches!(args.target, ConfigTarget::Auto | ConfigTarget::Toml) {
        let path = toml_file_path(args.toml_path.as_deref());
        let entries = list_toml_entries(&path)?;
        toml_entries = Some((path, entries));
    }

    if args.json {
        let mut env_json = serde_json::Map::new();
        let mut toml_json = serde_json::Map::new();
        if let Some((_, entries)) = &env_entries {
            for (k, v) in entries {
                env_json.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }
        if let Some((_, entries)) = &toml_entries {
            for (k, v) in entries {
                toml_json.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }
        let payload = serde_json::json!({
            "env": {
                "path": env_entries.as_ref().map(|(p, _)| p.display().to_string()),
                "values": env_json,
            },
            "toml": {
                "path": toml_entries.as_ref().map(|(p, _)| p.display().to_string()),
                "values": toml_json,
            },
        });
        print_json(&payload)?;
        return Ok(());
    }

    if let Some((path, entries)) = &env_entries {
        println!("# .env  ({})", path.display());
        if entries.is_empty() {
            println!("# (empty or missing)");
        } else {
            for (k, v) in entries {
                println!("{k}={v}");
            }
        }
        if toml_entries.is_some() {
            println!();
        }
    }
    if let Some((path, entries)) = &toml_entries {
        println!("# config.toml  ({})", path.display());
        if entries.is_empty() {
            println!("# (empty or missing)");
        } else {
            for (k, v) in entries {
                println!("{k} = {v}");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Routing + path resolution

fn resolve_target(key: &str, explicit: ConfigTarget) -> Result<ConfigTarget> {
    if !matches!(explicit, ConfigTarget::Auto) {
        return Ok(explicit);
    }
    if key.contains('.') {
        return Ok(ConfigTarget::Toml);
    }
    if looks_like_env_key(key) {
        return Ok(ConfigTarget::Env);
    }
    bail!(
        "could not infer target for key `{key}`: use a dotted TOML path (e.g. `syslog.host`), \
         an UPPER_CASE env var name, or pass --env / --toml explicitly"
    );
}

fn looks_like_env_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
}

fn validate_env_key(key: &str) -> Result<()> {
    if !looks_like_env_key(key) {
        bail!("invalid env key `{key}`: expected UPPER_CASE letters, digits, and underscores");
    }
    Ok(())
}

fn env_file_path() -> Result<PathBuf> {
    let home = cortex::setup::cortex_home_dir()
        .map_err(|e| anyhow!("could not determine syslog home for .env: {e}"))?;
    Ok(home.join(".env"))
}

fn toml_file_path(override_path: Option<&std::path::Path>) -> PathBuf {
    override_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

// ---------------------------------------------------------------------------
// .env read/write (comment-preserving, in-order)

fn read_env_kv(path: &std::path::Path, key: &str) -> Result<Option<String>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => bail!("failed to read {}: {e}", path.display()),
    };
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=')
            && k.trim() == key
        {
            return Ok(Some(v.trim().to_string()));
        }
    }
    Ok(None)
}

fn write_env_value(path: &std::path::Path, key: &str, value: &str) -> Result<()> {
    if value.contains('\n') || value.contains('\r') {
        bail!("env values cannot contain newlines");
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => bail!("failed to read {}: {e}", path.display()),
    };

    let mut out = String::new();
    let mut replaced = false;
    let mut had_trailing_newline = original.ends_with('\n');
    for line in original.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some((k, _)) = trimmed.split_once('=')
            && k.trim() == key
        {
            out.push_str(&format!("{key}={value}"));
            out.push('\n');
            replaced = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{key}={value}"));
        out.push('\n');
        had_trailing_newline = true;
    }
    if !had_trailing_newline && out.ends_with('\n') && original.is_empty() {
        // first-time write: keep the final newline
    }

    write_env_file(path, &out)
}

fn remove_env_value(path: &std::path::Path, key: &str) -> Result<Option<String>> {
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => bail!("failed to read {}: {e}", path.display()),
    };
    let mut out = String::new();
    let mut removed: Option<String> = None;
    for line in original.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=')
            && k.trim() == key
        {
            removed = Some(v.trim().to_string());
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if removed.is_some() {
        write_env_file(path, &out)?;
    }
    Ok(removed)
}

fn write_env_file(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let temp_path = atomic_write_path(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| anyhow!("failed to chmod {}: {e}", path.display()))?;
        }
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
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or(".env");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{file_name}.tmp.{}.{}", std::process::id(), count))
}

fn list_env_entries(path: &std::path::Path) -> Result<Vec<(String, String)>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => bail!("failed to read {}: {e}", path.display()),
    };
    let mut entries = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            entries.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Output helpers

fn print_config_value(
    key: &str,
    value: Option<&str>,
    target: &str,
    path: &std::path::Path,
    json: bool,
) -> Result<()> {
    if json {
        print_json(&serde_json::json!({
            "key": key,
            "value": value,
            "target": target,
            "path": path.display().to_string(),
            "found": value.is_some(),
        }))?;
        if value.is_none() {
            std::process::exit(1);
        }
        return Ok(());
    }
    match value {
        Some(v) => println!("{v}"),
        None => {
            eprintln!("{key} not set in {} ({})", path.display(), target);
            std::process::exit(1);
        }
    }
    Ok(())
}

fn print_config_set(
    key: &str,
    previous: Option<&str>,
    value: &str,
    target: &str,
    path: &std::path::Path,
    json: bool,
) -> Result<()> {
    if json {
        print_json(&serde_json::json!({
            "key": key,
            "previous": previous,
            "value": value,
            "target": target,
            "path": path.display().to_string(),
        }))?;
        return Ok(());
    }
    match previous {
        Some(prev) if prev != value => println!(
            "{key} = {value}  (was {prev}) [{target}: {}]",
            path.display()
        ),
        Some(_) => println!(
            "{key} = {value}  (unchanged) [{target}: {}]",
            path.display()
        ),
        None => println!("{key} = {value}  (new) [{target}: {}]", path.display()),
    }
    Ok(())
}

fn print_config_unset(
    key: &str,
    removed: Option<&str>,
    target: &str,
    path: &std::path::Path,
    json: bool,
) -> Result<()> {
    if json {
        print_json(&serde_json::json!({
            "key": key,
            "removed": removed,
            "target": target,
            "path": path.display().to_string(),
            "found": removed.is_some(),
        }))?;
        return Ok(());
    }
    match removed {
        Some(v) => println!("removed {key} (was {v}) [{target}: {}]", path.display()),
        None => {
            eprintln!("{key} not set in {} ({})", path.display(), target);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_cmd_tests.rs"]
mod tests;
