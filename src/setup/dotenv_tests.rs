use super::*;

#[test]
fn literal_values_survive_render_and_reparse() {
    for value in [
        "ordinary-safe_token123",
        "$EXPAND",
        "${EXPAND}",
        "$$dollars",
        "abc # comment",
        "#leading",
        "double\"quote",
        "single'quote",
        "back\\slash",
        "trailing\\",
        "\\\"both",
        "\\'both",
        " leading and trailing ",
        "tabs\t",
        "unicode 🚀",
        "\"$dollar # 'quoted' \\ end\\",
    ] {
        let raw = format!("CORTEX_TOKEN={}\n", encode(value));
        let parsed = parse(&raw);
        assert_eq!(parsed["CORTEX_TOKEN"], value, "encoded value: {raw:?}");
        assert_eq!(encode(&parsed["CORTEX_TOKEN"]), encode(value));
    }
}

#[test]
fn existing_quotes_comments_and_literal_dollars_are_decoded() {
    let raw = r#"
CORTEX_TOKEN='literal $DOLLAR # comment'
CORTEX_API_TOKEN="escaped \"quote\" and \\ slash and \$DOLLAR"
UNQUOTED=value # comment
HASH=literal#hash
EMPTY=
export EXPORTED='keep me'
"#;
    let parsed = parse(raw);
    assert_eq!(parsed["CORTEX_TOKEN"], "literal $DOLLAR # comment");
    assert_eq!(
        parsed["CORTEX_API_TOKEN"],
        "escaped \"quote\" and \\ slash and $DOLLAR"
    );
    assert_eq!(parsed["UNQUOTED"], "value");
    assert_eq!(parsed["HASH"], "literal#hash");
    assert_eq!(parsed["EMPTY"], "");
    assert_eq!(parsed["EXPORTED"], "keep me");
}

#[test]
fn entry_order_preserves_loader_duplicate_policy() {
    let values = entries("TOKEN='first'\nTOKEN=\"last\"\n");
    assert_eq!(
        values,
        vec![
            ("TOKEN".into(), "first".into()),
            ("TOKEN".into(), "last".into())
        ]
    );
    assert_eq!(parse("TOKEN='first'\nTOKEN=\"last\"\n")["TOKEN"], "last");
}

#[test]
fn unchanged_existing_interpolation_and_quotes_are_preserved_verbatim() {
    let existing = "CORTEX_WORKSPACE_VOLUME=${HOME}/workspace\nCORTEX_TOKEN=$TOKEN_FROM_OPERATOR\nCORTEX_API_TOKEN='literal $TOKEN # comments'\n";
    let values = parse(existing);
    let repaired = render_preserving(existing, &values);
    assert!(repaired.contains("CORTEX_WORKSPACE_VOLUME=${HOME}/workspace\n"));
    assert!(repaired.contains("CORTEX_TOKEN=$TOKEN_FROM_OPERATOR\n"));
    assert!(repaired.contains("CORTEX_API_TOKEN='literal $TOKEN # comments'\n"));
    assert_eq!(render_preserving(&repaired, &parse(&repaired)), repaired);
    let mut changed = values;
    changed.insert("CORTEX_TOKEN".into(), "$new-literal".into());
    assert!(render_preserving(existing, &changed).contains("CORTEX_TOKEN=\"\\$new-literal\""));
}
