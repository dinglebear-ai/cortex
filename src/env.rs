//! Process-environment access with a test-only overlay.
//!
//! Rust 2024 makes process-environment mutation unsafe on platforms where
//! libc environment reads can race with writes. Cortex never needs to mutate
//! its own environment in production. Tests exercise env precedence through a
//! Rust map instead of the process environment, removing that unsafe boundary.

use std::ffi::{OsStr, OsString};

#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "test-support"))]
use std::sync::{OnceLock, RwLock};

// Binary and library tests share this single map through the `test-support`
// feature. Production builds compile none of the override storage or setters.
#[cfg(any(test, feature = "test-support"))]
static TEST_OVERRIDES: OnceLock<RwLock<HashMap<OsString, Option<OsString>>>> = OnceLock::new();

#[doc(hidden)]
#[inline]
pub fn var<K: AsRef<OsStr>>(key: K) -> Result<String, std::env::VarError> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(value) = test_override(key.as_ref()) {
        return match value {
            Some(value) => value.into_string().map_err(std::env::VarError::NotUnicode),
            None => Err(std::env::VarError::NotPresent),
        };
    }
    std::env::var(key)
}

#[doc(hidden)]
#[inline]
pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(value) = test_override(key.as_ref()) {
        return value;
    }
    std::env::var_os(key)
}

#[cfg(any(test, feature = "test-support"))]
fn test_override(key: &OsStr) -> Option<Option<OsString>> {
    TEST_OVERRIDES
        .get()?
        .read()
        .expect("test environment overlay lock poisoned")
        .get(key)
        .cloned()
}

/// Set a test-only environment override without mutating the process environment.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn set_test_var<K, V>(key: K, value: V)
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    TEST_OVERRIDES
        .get_or_init(|| RwLock::new(HashMap::new()))
        .write()
        .expect("test environment overlay lock poisoned")
        .insert(
            key.as_ref().to_os_string(),
            Some(value.as_ref().to_os_string()),
        );
}

/// Mask a key from the real process environment for tests.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn remove_test_var<K: AsRef<OsStr>>(key: K) {
    TEST_OVERRIDES
        .get_or_init(|| RwLock::new(HashMap::new()))
        .write()
        .expect("test environment overlay lock poisoned")
        .insert(key.as_ref().to_os_string(), None);
}

/// Resolve an executable only when tests explicitly override `PATH`.
/// Normal test-support builds keep the platform's native command lookup semantics.
#[cfg(any(test, feature = "test-support"))]
fn resolve_test_program(program: &OsStr) -> Option<std::path::PathBuf> {
    let program_path = std::path::Path::new(program);
    if program_path.components().count() != 1 {
        return None;
    }
    let search_path = test_override(OsStr::new("PATH"))??;
    std::env::split_paths(&search_path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg(any(test, feature = "test-support"))]
fn test_overrides_snapshot() -> Vec<(OsString, Option<OsString>)> {
    TEST_OVERRIDES
        .get()
        .map(|overrides| {
            overrides
                .read()
                .expect("test environment overlay lock poisoned")
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Construct a blocking subprocess command using the real process environment
/// in production and the safe test overlay in test-support builds.
#[doc(hidden)]
#[inline]
pub fn command<P: AsRef<OsStr>>(program: P) -> std::process::Command {
    #[cfg(any(test, feature = "test-support"))]
    {
        let program = program.as_ref();
        let program_path = std::path::Path::new(program);
        let resolved = resolve_test_program(program);
        let mut command = std::process::Command::new(resolved.as_deref().unwrap_or(program_path));
        for (key, value) in test_overrides_snapshot() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        command
    }

    #[cfg(not(any(test, feature = "test-support")))]
    std::process::Command::new(program)
}

/// Construct an async Tokio subprocess command with the same test overlay
/// semantics as [`command`].
#[doc(hidden)]
#[inline]
pub fn tokio_command<P: AsRef<OsStr>>(program: P) -> tokio::process::Command {
    #[cfg(any(test, feature = "test-support"))]
    {
        let program = program.as_ref();
        let program_path = std::path::Path::new(program);
        let resolved = resolve_test_program(program);
        let mut command = tokio::process::Command::new(resolved.as_deref().unwrap_or(program_path));
        for (key, value) in test_overrides_snapshot() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        command
    }

    #[cfg(not(any(test, feature = "test-support")))]
    tokio::process::Command::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_override_can_set_and_mask_a_key_without_process_mutation() {
        const KEY: &str = "CORTEX_TEST_ENV_OVERLAY_SET_AND_MASK";
        set_test_var(KEY, "overlay-value");
        assert_eq!(var(KEY).as_deref(), Ok("overlay-value"));
        assert_eq!(var_os(KEY).as_deref(), Some(OsStr::new("overlay-value")));

        remove_test_var(KEY);
        assert!(matches!(var(KEY), Err(std::env::VarError::NotPresent)));
        assert!(var_os(KEY).is_none());
    }

    #[test]
    fn unrelated_keys_do_not_interfere() {
        const LEFT: &str = "CORTEX_TEST_ENV_OVERLAY_LEFT";
        const RIGHT: &str = "CORTEX_TEST_ENV_OVERLAY_RIGHT";
        set_test_var(LEFT, "left");
        set_test_var(RIGHT, "right");
        assert_eq!(var(LEFT).as_deref(), Ok("left"));
        assert_eq!(var(RIGHT).as_deref(), Ok("right"));
    }
}
