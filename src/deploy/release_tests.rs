use super::*;
#[test]
fn stable_resolver_rejects_wrong_repository_and_unpublished_targets() {
    let valid = br#"{"tag_name":"v4.2.1","draft":false,"prerelease":false,"html_url":"https://github.com/dinglebear-ai/cortex/releases/tag/v4.2.1"}"#;
    assert_eq!(parse_release(valid).unwrap(), "4.2.1");
    let raw = String::from_utf8(valid.to_vec()).unwrap();
    assert!(parse_release(raw.replace("\"draft\":false", "\"draft\":true").as_bytes()).is_err());
    assert!(
        parse_release(
            raw.replace("\"prerelease\":false", "\"prerelease\":true")
                .as_bytes()
        )
        .is_err()
    );
    assert!(parse_release(raw.replace("dinglebear-ai", "other-owner").as_bytes()).is_err());
    assert!(parse_release(raw.replace("v4.2.1", "v4.2.1;bad").as_bytes()).is_err());
}
