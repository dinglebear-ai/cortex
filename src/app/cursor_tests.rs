use super::*;

#[test]
fn cursor_round_trips_without_padding() {
    let filters = filter_fingerprint(&serde_json::json!({"host":"dookie"})).unwrap();
    let cursor = PageCursor {
        sort: "2026-08-21T12:00:00Z".into(),
        id: 42,
        direction: CursorDirection::Desc,
        filters: filters.clone(),
    };
    let encoded = encode_cursor(&cursor).unwrap();
    assert!(!encoded.contains('='));
    assert_eq!(
        decode_cursor(&encoded, &filters, CursorDirection::Desc).unwrap(),
        cursor
    );
}

#[test]
fn cursor_rejects_tampering_and_filter_or_direction_reuse() {
    let filters = filter_fingerprint(&serde_json::json!({"host":"dookie"})).unwrap();
    let cursor = PageCursor {
        sort: "9".into(),
        id: 7,
        direction: CursorDirection::Asc,
        filters: filters.clone(),
    };
    let encoded = encode_cursor(&cursor).unwrap();
    let mut tampered = encoded.clone();
    tampered.replace_range(2..3, "!");
    assert_eq!(
        decode_cursor(&tampered, &filters, CursorDirection::Asc),
        Err(CursorError::Invalid)
    );
    assert_eq!(
        decode_cursor(&encoded, "different", CursorDirection::Asc),
        Err(CursorError::FilterMismatch)
    );
    assert_eq!(
        decode_cursor(&encoded, &filters, CursorDirection::Desc),
        Err(CursorError::Invalid)
    );
}
