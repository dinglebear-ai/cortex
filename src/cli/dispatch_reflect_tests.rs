use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;

#[test]
fn flag_wins_over_env_and_home() {
    let (path, source) = resolve_reflect_db_path(
        Some(Path::new("/flag.db")),
        Some(OsString::from("/env.db")),
        Some(OsString::from("/home/u")),
    )
    .unwrap();
    assert_eq!(
        (path, source),
        (PathBuf::from("/flag.db"), ReflectDbSource::Flag)
    );
}

#[test]
fn env_wins_over_home() {
    let (path, source) = resolve_reflect_db_path(
        None,
        Some(OsString::from("/env.db")),
        Some(OsString::from("/home/u")),
    )
    .unwrap();
    assert_eq!(
        (path, source),
        (PathBuf::from("/env.db"), ReflectDbSource::Env)
    );
}

#[test]
fn empty_env_falls_back_to_home_default() {
    let (path, source) =
        resolve_reflect_db_path(None, Some(OsString::new()), Some(OsString::from("/home/u")))
            .unwrap();
    assert_eq!(
        (path, source),
        (
            PathBuf::from("/home/u/.cortex/reflect.db"),
            ReflectDbSource::Default
        )
    );
}

#[test]
fn missing_home_is_an_error_that_mentions_db_flag() {
    let error = resolve_reflect_db_path(None, None, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("--db"), "{error}");
}

#[cfg(unix)]
#[test]
fn default_parent_is_created_owner_only_and_existing_dirs_are_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join(".cortex/reflect.db");
    prepare_reflect_db_dir(&db, ReflectDbSource::Default).unwrap();
    let mode = std::fs::metadata(home.path().join(".cortex"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);

    let existing = tempfile::tempdir().unwrap();
    std::fs::set_permissions(existing.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    prepare_reflect_db_dir(
        &existing.path().join("reflect.db"),
        ReflectDbSource::Default,
    )
    .unwrap();
    let mode = std::fs::metadata(existing.path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[cfg(unix)]
#[test]
fn db_file_and_wal_siblings_become_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("reflect.db");
    for name in ["reflect.db", "reflect.db-wal"] {
        std::fs::write(dir.path().join(name), "").unwrap();
        std::fs::set_permissions(
            dir.path().join(name),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
    }
    restrict_reflect_db_file(&db).unwrap();
    for name in ["reflect.db", "reflect.db-wal"] {
        let mode = std::fs::metadata(dir.path().join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
}

#[test]
fn reflect_storage_disables_db_size_self_trim() {
    // The reflect DB is a rebuildable cache. The server's DB-size self-trim
    // deletes the oldest rows on every indexing chunk once the file passes
    // max_db_size_mb, which stalls indexing and drops transcript rows.
    let mut storage = cortex::config::StorageConfig::default();
    storage.max_db_size_mb = 1024;
    storage.recovery_db_size_mb = 900;
    storage.min_free_disk_mb = 512;
    storage.recovery_free_disk_mb = 1024;
    let reflect = reflect_storage_config(storage);
    assert_eq!(reflect.max_db_size_mb, 0);
    assert_eq!(reflect.recovery_db_size_mb, 0);
    // The free-disk guard blocks writes instead of deleting; keep it.
    assert_eq!(reflect.min_free_disk_mb, 512);
    assert_eq!(reflect.recovery_free_disk_mb, 1024);
}
