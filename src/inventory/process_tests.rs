use super::*;

#[tokio::test]
async fn command_timeout_returns_error() {
    for attempt in 0..5 {
        match run_command(
            "sh",
            &["-c", "while :; do :; done"],
            Duration::from_millis(50),
        )
        .await
        {
            Err(error) if error.to_string().contains("spawn failed") && attempt < 4 => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => {
                assert!(
                    error.to_string().contains("timed out"),
                    "unexpected command error: {error:#}"
                );
                return;
            }
            Ok(output) => panic!("non-terminating command unexpectedly exited: {output:?}"),
        }
    }
    unreachable!("timeout command retry loop always returns or panics")
}

#[test]
fn shell_words_splits_simple_commands() {
    assert_eq!(
        shell_words("git status --porcelain"),
        vec!["git", "status", "--porcelain"]
    );
}

#[test]
fn shell_words_preserves_quoted_and_escaped_arguments() {
    assert_eq!(
        shell_words(r#"sh -c 'echo hello world' path\ with\ spaces"#),
        vec!["sh", "-c", "echo hello world", "path with spaces"]
    );
    assert_eq!(
        shell_words(r#"cmd "quoted \"inner\" value""#),
        vec!["cmd", r#"quoted "inner" value"#]
    );
}

#[tokio::test]
async fn byte_command_output_preserves_non_utf8_stdout() {
    let output =
        run_command_bytes_capped("sh", &["-c", r"printf '\377'"], Duration::from_secs(1), 16)
            .await
            .unwrap();
    assert_eq!(output.stdout, vec![255]);
    assert!(!output.truncated);
}
