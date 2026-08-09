use std::path::PathBuf;
use std::process::Command;

const S3_ENV: [&str; 5] = [
    "KACHE_S3_ACCESS_KEY",
    "KACHE_S3_SECRET_KEY",
    "KACHE_S3_ENDPOINT",
    "KACHE_S3_BUCKET",
    "KACHE_S3_PREFIX",
];

fn repo_path(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn mode_for_present_fields(present: u8) -> String {
    let mut command = Command::new("bash");
    command
        .arg(repo_path("scripts/ci/kache-s3-mode.sh"))
        .env_clear();
    for (index, name) in S3_ENV.iter().enumerate() {
        if present & (1 << index) != 0 {
            command.env(name, format!("value-{index}"));
        }
    }
    let output = command.output().expect("kache S3 mode helper must run");
    assert!(output.status.success(), "mode helper must succeed");
    String::from_utf8(output.stdout)
        .expect("mode must be UTF-8")
        .trim()
        .to_owned()
}

#[test]
fn kache_remote_requires_every_nonempty_s3_field() {
    let complete = (1 << S3_ENV.len()) - 1;
    for present in 0..=complete {
        let expected = if present == complete {
            "remote"
        } else {
            "local"
        };
        assert_eq!(
            mode_for_present_fields(present as u8),
            expected,
            "unexpected mode for field bitset {present:05b}"
        );
    }
}

#[test]
fn release_uses_action_defaults_for_bucket_and_prefix() {
    let action = std::fs::read_to_string(repo_path(".github/actions/setup-rust-kache/action.yml"))
        .expect("setup action must be readable");
    let release = std::fs::read_to_string(repo_path(".github/workflows/release.yml"))
        .expect("release workflow must be readable");

    assert!(
        action.contains("default: \"kache\"") && action.contains("default: \"rust\""),
        "the action must retain safe nonempty bucket and prefix defaults"
    );
    for required in ["s3-access-key:", "s3-secret-key:", "s3-endpoint:"] {
        assert!(
            release.contains(required),
            "release workflow must pass {required}"
        );
    }
    for defaulted in ["s3-bucket:", "s3-prefix:"] {
        assert!(
            !release.contains(defaulted),
            "release workflow must not override the action's {defaulted} default with a possibly-empty org variable"
        );
    }
    assert!(
        action.contains("scripts/ci/kache-s3-mode.sh") && action.contains(")\" = remote ]; then"),
        "the action must gate remote configuration through the complete-tuple helper"
    );
}
