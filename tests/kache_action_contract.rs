use std::path::PathBuf;
use std::process::Command;

use serde_yaml_ng::Value;

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

fn mode_for_values(values: &[Option<&str>; 5]) -> String {
    let mut command = Command::new("bash");
    command
        .arg(repo_path("scripts/ci/kache-s3-mode.sh"))
        .env_clear();
    for (name, value) in S3_ENV.iter().zip(values) {
        if let Some(value) = value {
            command.env(name, value);
        }
    }
    let output = command.output().expect("kache S3 mode helper must run");
    assert!(output.status.success(), "mode helper must succeed");
    String::from_utf8(output.stdout)
        .expect("mode must be UTF-8")
        .trim()
        .to_owned()
}

fn mode_for_present_fields(present: u8) -> String {
    let mut values = [None; 5];
    for (index, value) in values.iter_mut().enumerate() {
        if present & (1 << index) != 0 {
            *value = Some("value");
        }
    }
    mode_for_values(&values)
}

fn yaml(path: &str) -> Value {
    serde_yaml_ng::from_str(
        &std::fs::read_to_string(repo_path(path)).expect("YAML file must be readable"),
    )
    .expect("YAML file must parse")
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
fn kache_remote_rejects_unsafe_values_in_every_s3_field() {
    let valid = [
        "AKIAEXAMPLE123",
        "secret/+==._:-",
        "https://s3.example.test",
        "kache-cache",
        "rust/v1",
    ];
    assert_eq!(
        mode_for_values(&valid.map(Some)),
        "remote",
        "realistic punctuation must remain valid"
    );

    for invalid in ["", " ", "\t", "\n", "\r", "bad\"value", "bad\\value"] {
        for index in 0..S3_ENV.len() {
            let mut values = valid.map(Some);
            values[index] = Some(invalid);
            assert_eq!(
                mode_for_values(&values),
                "local",
                "{} must reject hostile value {invalid:?}",
                S3_ENV[index]
            );
        }
    }
}

#[test]
fn release_and_action_have_exact_s3_wiring() {
    let action = yaml(".github/actions/setup-rust-kache/action.yml");
    let release = yaml(".github/workflows/release.yml");

    assert_eq!(
        action["inputs"]["s3-bucket"]["default"].as_str(),
        Some("kache")
    );
    assert_eq!(
        action["inputs"]["s3-prefix"]["default"].as_str(),
        Some("rust")
    );

    let release_steps = release["jobs"]["cortex-linux"]["steps"]
        .as_sequence()
        .expect("cortex-linux steps must be a sequence");
    let release_setup = release_steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Install Rust and kache"))
        .expect("release must have a named Kache setup step");
    assert_eq!(
        release_setup["uses"].as_str(),
        Some("./.github/actions/setup-rust-kache")
    );
    let release_with = release_setup["with"]
        .as_mapping()
        .expect("release Kache step must have inputs");
    assert_eq!(release_with.len(), 3, "release must rely on both defaults");
    for (name, expression) in [
        ("s3-access-key", "${{ secrets.KACHE_S3_ACCESS_KEY }}"),
        ("s3-secret-key", "${{ secrets.KACHE_S3_SECRET_KEY }}"),
        ("s3-endpoint", "${{ vars.KACHE_S3_ENDPOINT }}"),
    ] {
        assert_eq!(
            release_setup["with"][name].as_str(),
            Some(expression),
            "release input {name} must use the expected GitHub context"
        );
    }

    let action_steps = action["runs"]["steps"]
        .as_sequence()
        .expect("composite action steps must be a sequence");
    let configure = action_steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Configure kache"))
        .expect("action must have a Configure kache step");
    let configure_env = configure["env"]
        .as_mapping()
        .expect("Configure kache must map its inputs into env");
    assert_eq!(configure_env.len(), S3_ENV.len());
    for (env_name, input_name) in [
        ("KACHE_S3_ACCESS_KEY", "s3-access-key"),
        ("KACHE_S3_SECRET_KEY", "s3-secret-key"),
        ("KACHE_S3_ENDPOINT", "s3-endpoint"),
        ("KACHE_S3_BUCKET", "s3-bucket"),
        ("KACHE_S3_PREFIX", "s3-prefix"),
    ] {
        let expected = format!("${{{{ inputs.{input_name} }}}}");
        assert_eq!(configure["env"][env_name].as_str(), Some(expected.as_str()));
    }
    assert!(
        configure["run"]
            .as_str()
            .is_some_and(|run| run.contains("scripts/ci/kache-s3-mode.sh")
                && run.contains(")\" = remote ]; then")),
        "the action must gate remote configuration through the complete-tuple helper"
    );
}
