use super::*;
use std::io::Write;
fn append(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
}
#[tokio::test]
async fn partial_lines_and_split_utf8_wait_for_delimiter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, b"old\n").unwrap();
    let mut tail = TailReader::open(&path).await.unwrap();
    append(&path, b"request \xc3");
    assert_eq!(tail.next_line().await.unwrap(), None);
    append(&path, b"\xa9nded\n");
    assert_eq!(
        tail.next_line().await.unwrap().as_deref(),
        Some("request énded")
    );
    assert_eq!(tail.next_line().await.unwrap(), None);
}
#[tokio::test]
async fn replacement_larger_than_offset_starts_at_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, b"old\n").unwrap();
    let mut tail = TailReader::open(&path).await.unwrap();
    std::fs::rename(&path, dir.path().join("old.log")).unwrap();
    std::fs::write(&path, b"replacement starts here\n").unwrap();
    assert_eq!(tail.next_line().await.unwrap(), None);
    assert_eq!(
        tail.next_line().await.unwrap().as_deref(),
        Some("replacement starts here")
    );
}
#[tokio::test]
async fn copytruncate_preserves_replacement_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, b"long old record\n").unwrap();
    let mut tail = TailReader::open(&path).await.unwrap();
    std::fs::write(&path, b"new\n").unwrap();
    assert_eq!(tail.next_line().await.unwrap().as_deref(), Some("new"));
}
#[tokio::test]
async fn oversized_partial_buffer_is_bounded_and_next_record_survives() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, b"").unwrap();
    let mut tail = TailReader::open(&path).await.unwrap();
    append(&path, &vec![b'x'; MAX_LINE_BYTES + 100]);
    assert_eq!(tail.next_line().await.unwrap(), None);
    assert_eq!(tail.partial.len(), MAX_LINE_BYTES);
    append(&path, b"\nnext\n");
    assert_eq!(
        tail.next_line().await.unwrap().unwrap().len(),
        MAX_LINE_BYTES
    );
    assert_eq!(tail.next_line().await.unwrap().as_deref(), Some("next"));
}

#[tokio::test]
async fn regrown_copytruncate_checks_prefix_before_reading_old_offset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, b"old\n").unwrap();
    let mut tail = TailReader::open(&path).await.unwrap();
    append(&path, b"growing old\n");
    assert_eq!(
        tail.next_line().await.unwrap().as_deref(),
        Some("growing old")
    );
    assert_eq!(tail.next_line().await.unwrap(), None);
    assert!(tail.prefix.len() > 4);
    std::fs::write(
        &path,
        b"old\nnew replacement longer than previous contents\n",
    )
    .unwrap();
    assert_eq!(tail.next_line().await.unwrap().as_deref(), Some("old"));
    assert_eq!(
        tail.next_line().await.unwrap().as_deref(),
        Some("new replacement longer than previous contents")
    );
}
