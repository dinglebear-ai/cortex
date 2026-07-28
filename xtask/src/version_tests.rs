use super::*;

#[test]
fn reads_cargo_package_version() {
    let toml = "[package]\nname = \"cortex\"\nversion = \"1.2.3\"\n";
    assert_eq!(
        read_cargo_package_version(toml, Some("cortex")).unwrap(),
        "1.2.3"
    );
}

#[test]
fn cargo_package_wrong_name_errors() {
    let toml = "[package]\nname = \"other\"\nversion = \"1.2.3\"\n";
    assert!(read_cargo_package_version(toml, Some("cortex")).is_err());
}

#[test]
fn reads_cargo_lock_package_version() {
    let lock = "[[package]]\nname = \"foo\"\nversion = \"9.9.9\"\n\n[[package]]\nname = \"cortex\"\nversion = \"1.2.3\"\n";
    assert_eq!(
        read_cargo_lock_package_version(lock, Some("cortex")).unwrap(),
        "1.2.3"
    );
}

#[test]
fn reads_json_pointer_version() {
    let json = r#"{"version":"1.2.3","info":{"version":"4.5.6"}}"#;
    assert_eq!(read_json_version(json, Some("/version")).unwrap(), "1.2.3");
    assert_eq!(
        read_json_version(json, Some("/info/version")).unwrap(),
        "4.5.6"
    );
}

#[test]
fn reads_regex_version() {
    let body = "image: ghcr.io/dinglebear-ai/cortex:v1.22.0\n";
    assert_eq!(
        read_regex_version(body, Some(r"cortex:v(\d+\.\d+\.\d+)")).unwrap(),
        "1.22.0"
    );
}

#[test]
fn replace_cargo_package_only_touches_package_table() {
    let toml =
        "[package]\nname = \"cortex\"\nversion = \"1.2.3\"\n\n[dependencies]\nversion = \"0.1\"\n";
    let out = replace_cargo_package_version(toml, Some("cortex"), "2.0.0").unwrap();
    assert!(out.contains("[package]\nname = \"cortex\"\nversion = \"2.0.0\""));
    // The unrelated [dependencies] `version` key is left alone.
    assert!(out.contains("[dependencies]\nversion = \"0.1\""));
}

#[test]
fn replace_cargo_lock_targets_named_package() {
    let lock = "[[package]]\nname = \"foo\"\nversion = \"9.9.9\"\n\n[[package]]\nname = \"cortex\"\nversion = \"1.2.3\"\n";
    let out = replace_cargo_lock_package_version(lock, Some("cortex"), "2.0.0").unwrap();
    assert!(out.contains("name = \"cortex\"\nversion = \"2.0.0\""));
    assert!(out.contains("name = \"foo\"\nversion = \"9.9.9\""));
}

#[test]
fn replace_json_version_preserves_other_fields() {
    let json = "{\n  \"version\": \"1.2.3\",\n  \"name\": \"cortex\"\n}\n";
    let out = replace_json_version(json, Some("/version"), "2.0.0").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["version"], "2.0.0");
    assert_eq!(parsed["name"], "cortex");
    assert!(out.ends_with('\n'));
}

#[test]
fn replace_json_version_preserves_key_order() {
    // Keys deliberately NOT in alphabetical order. Without serde_json's
    // preserve_order feature this round-trips through a BTreeMap and comes back
    // sorted ($schema, name, version -> alphabetized), producing a huge diff.
    let json = "{\n  \"$schema\": \"x\",\n  \"name\": \"cortex\",\n  \"version\": \"1.2.3\",\n  \"packages\": []\n}\n";
    let out = replace_json_version(json, Some("/version"), "2.0.0").unwrap();
    let schema = out.find("\"$schema\"").unwrap();
    let name = out.find("\"name\"").unwrap();
    let version = out.find("\"version\"").unwrap();
    let packages = out.find("\"packages\"").unwrap();
    assert!(
        schema < name && name < version && version < packages,
        "key order must be preserved, got:\n{out}"
    );
    assert!(out.contains("\"version\": \"2.0.0\""));
}

#[test]
fn replace_regex_version_replaces_all_occurrences() {
    let body = "a cortex:v1.0.0 b cortex:v1.0.0\n";
    let out = replace_regex_version(body, Some(r"cortex:v(\d+\.\d+\.\d+)"), "2.3.4").unwrap();
    assert_eq!(out, "a cortex:v2.3.4 b cortex:v2.3.4\n");
}

#[test]
fn replace_regex_version_errors_when_absent() {
    assert!(replace_regex_version("nope", Some(r"cortex:v(\d+\.\d+\.\d+)"), "2.0.0").is_err());
}

#[test]
fn json_no_top_level_version_detects_key() {
    assert!(check_json_no_top_level_version(r#"{"version":"1.0.0"}"#).is_err());
    assert!(check_json_no_top_level_version(r#"{"name":"x"}"#).is_ok());
    // Nested version keys are allowed — only the top level is forbidden.
    assert!(check_json_no_top_level_version(r#"{"servers":{"a":{"version":"1"}}}"#).is_ok());
}

#[test]
fn changelog_heading_check() {
    let log = "# Changelog\n\n## [1.2.3] - 2026-01-01\n";
    assert!(check_changelog_heading(log, "1.2.3").is_ok());
    assert!(check_changelog_heading(log, "1.2.4").is_err());
}

#[test]
fn ensure_changelog_inserts_under_unreleased() {
    let log = "# Changelog\n\n## [Unreleased]\n\n## [1.2.3] - 2026-01-01\n";
    let out = ensure_changelog_heading(log, "1.3.0").unwrap();
    let unreleased = out.find("## [Unreleased]").unwrap();
    let new = out.find("## [1.3.0]").unwrap();
    let old = out.find("## [1.2.3]").unwrap();
    assert!(
        unreleased < new && new < old,
        "new entry sits between Unreleased and prior release"
    );
}

#[test]
fn ensure_changelog_is_idempotent() {
    let log = "# Changelog\n\n## [Unreleased]\n\n## [1.3.0] - 2026-01-01\n";
    assert_eq!(ensure_changelog_heading(log, "1.3.0").unwrap(), log);
}

#[test]
fn bump_round_trip_on_a_temp_manifest() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("release")).unwrap();
    fs::write(
        root.join("release/components.toml"),
        r#"schema_version = 1
[[components]]
id = "cortex"
name = "Cortex"
tag_prefix = "v"
version_source = { kind = "cargo_package", path = "Cargo.toml", package = "cortex" }
version_files = [
  { kind = "cargo_package", path = "Cargo.toml", package = "cortex" },
  { kind = "json_version", path = "server.json", json_pointer = "/version" },
  { kind = "regex_version", path = "server.json", pattern = 'cortex:v(\d+\.\d+\.\d+)' },
  { kind = "changelog_heading", path = "CHANGELOG.md" },
  { kind = "json_no_version", path = "plugin.json" },
]
"#,
    )
    .unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"cortex\"\nversion = \"1.2.3\"\n",
    )
    .unwrap();
    fs::write(
        root.join("server.json"),
        "{\n  \"version\": \"1.2.3\",\n  \"image\": \"ghcr.io/dinglebear-ai/cortex:v1.2.3\"\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n## [1.2.3] - 2026-01-01\n",
    )
    .unwrap();
    fs::write(root.join("plugin.json"), "{\n  \"name\": \"cortex\"\n}\n").unwrap();

    // Out of sync before bump? No — everything is 1.2.3.
    check_sync(root).unwrap();

    bump(root, BumpLevel::Minor).unwrap();

    assert_eq!(
        read_cargo_package_version(
            &fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            Some("cortex")
        )
        .unwrap(),
        "1.3.0"
    );
    let server = fs::read_to_string(root.join("server.json")).unwrap();
    assert!(server.contains("cortex:v1.3.0"));
    assert_eq!(
        read_json_version(&server, Some("/version")).unwrap(),
        "1.3.0"
    );

    // Everything is consistent and the changelog gained an entry.
    check_release(root).unwrap();
}

#[test]
fn sync_version_applies_canonical_source_to_lagging_files() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("release")).unwrap();
    fs::write(
        root.join("release/components.toml"),
        r#"schema_version = 1
[[components]]
id = "cortex"
name = "Cortex"
tag_prefix = "v"
version_source = { kind = "cargo_package", path = "Cargo.toml", package = "cortex" }
version_files = [
  { kind = "cargo_package", path = "Cargo.toml", package = "cortex" },
  { kind = "json_version", path = "server.json", json_pointer = "/version" },
  { kind = "regex_version", path = "server.json", pattern = 'cortex:v(\d+\.\d+\.\d+)' },
  { kind = "changelog_heading", path = "CHANGELOG.md" },
  { kind = "json_no_version", path = "plugin.json" },
]
"#,
    )
    .unwrap();
    // Simulate release-please's native `rust` strategy having already bumped
    // Cargo.toml and CHANGELOG.md, while server.json (a regex_version carrier
    // release-please's schema can't reach) still lags at the old version.
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"cortex\"\nversion = \"1.3.0\"\n",
    )
    .unwrap();
    fs::write(
        root.join("server.json"),
        "{\n  \"version\": \"1.2.3\",\n  \"image\": \"ghcr.io/dinglebear-ai/cortex:v1.2.3\"\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n## [1.3.0] - 2026-01-01\n",
    )
    .unwrap();
    fs::write(root.join("plugin.json"), "{\n  \"name\": \"cortex\"\n}\n").unwrap();

    assert!(check_sync(root).is_err(), "server.json should lag at first");

    sync_version(root).unwrap();

    let server = fs::read_to_string(root.join("server.json")).unwrap();
    assert!(server.contains("cortex:v1.3.0"));
    assert_eq!(
        read_json_version(&server, Some("/version")).unwrap(),
        "1.3.0"
    );
    check_release(root).unwrap();

    // Idempotent: running again makes no further changes and stays in sync.
    sync_version(root).unwrap();
    check_release(root).unwrap();
}

#[test]
fn drift_is_detected() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("release")).unwrap();
    fs::write(
        root.join("release/components.toml"),
        r#"schema_version = 1
[[components]]
id = "cortex"
name = "Cortex"
tag_prefix = "v"
version_source = { kind = "cargo_package", path = "Cargo.toml", package = "cortex" }
version_files = [
  { kind = "cargo_package", path = "Cargo.toml", package = "cortex" },
  { kind = "json_version", path = "server.json", json_pointer = "/version" },
]
"#,
    )
    .unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"cortex\"\nversion = \"1.3.0\"\n",
    )
    .unwrap();
    fs::write(root.join("server.json"), "{\n  \"version\": \"1.2.0\"\n}\n").unwrap();

    let err = check_sync(root).unwrap_err();
    assert!(err.to_string().contains("version check failed"));
}
