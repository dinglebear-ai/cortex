use super::*;

#[test]
fn source_id_is_uri_safe_and_stable() {
    assert_eq!(source_id("plex media/server"), "plex-media-server");
    assert_eq!(source_id("---"), "file-tail");
}

#[test]
fn delivery_keys_distinguish_identical_records_at_the_same_timestamp() {
    let config = FileTailForwardConfig {
        source: FileTailSource {
            path: "/var/log/app.log".into(),
            tag: Some("app".into()),
        },
        target: "http://cortex.test".into(),
        token: None,
        hostname: "host-a".into(),
    };

    let first = delivery_key(
        &config,
        "app",
        "2026-09-13T12:00:00.000Z",
        "same",
        1_000,
        41,
    );
    let second = delivery_key(
        &config,
        "app",
        "2026-09-13T12:00:00.000Z",
        "same",
        1_000,
        42,
    );

    assert_ne!(first, second);
    assert_eq!(
        first,
        delivery_key(
            &config,
            "app",
            "2026-09-13T12:00:00.000Z",
            "same",
            1_000,
            41,
        )
    );
}
