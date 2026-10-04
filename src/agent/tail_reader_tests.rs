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
        tail.next_line().await.unwrap().unwrap(),
        "[Cortex omitted oversized file-tail record: exceeds 65536-byte limit]"
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

#[tokio::test]
async fn oversized_unterminated_record_is_diagnosed_on_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, b"").unwrap();
    let mut tail = TailReader::open(&path).await.unwrap();
    append(&path, &vec![b'x'; MAX_LINE_BYTES + 1]);
    assert_eq!(tail.next_line().await.unwrap(), None);
    assert_eq!(tail.next_line().await.unwrap(), None);
    std::fs::rename(&path, dir.path().join("old.log")).unwrap();
    std::fs::write(&path, b"new record\n").unwrap();
    assert_eq!(
        tail.next_line().await.unwrap().as_deref(),
        Some("[Cortex omitted oversized file-tail record: exceeds 65536-byte limit]")
    );
    assert_eq!(
        tail.next_line().await.unwrap().as_deref(),
        Some("new record")
    );
}

#[tokio::test]
async fn durable_cursor_resumes_partial_line_and_appends_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, "initial history\n").unwrap();
    let mut reader = TailReader::open_durable(&path).await.unwrap();
    append(&path, b"partial ");
    assert_eq!(reader.next_line().await.unwrap(), None);
    let saved = reader.cursor().await.unwrap();
    drop(reader);
    append(&path, b"completed\nnext\n");
    let mut resumed = TailReader::restore(&path, saved).await.unwrap();
    assert_eq!(
        resumed.next_line().await.unwrap().as_deref(),
        Some("partial completed")
    );
    assert_eq!(resumed.next_line().await.unwrap().as_deref(), Some("next"));
}

#[tokio::test]
async fn durable_cursor_drains_rotated_inode_then_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, "initial history\n").unwrap();
    let mut reader = TailReader::open_durable(&path).await.unwrap();
    let saved = reader.cursor().await.unwrap();
    drop(reader);
    append(&path, b"old inode offline backlog\n");
    std::fs::rename(&path, dir.path().join("app.log.1")).unwrap();
    std::fs::write(&path, "replacement backlog\n").unwrap();
    let mut resumed = TailReader::restore(&path, saved).await.unwrap();
    assert_eq!(
        resumed.next_line().await.unwrap().as_deref(),
        Some("old inode offline backlog")
    );
    assert_eq!(resumed.next_line().await.unwrap(), None);
    assert_eq!(
        resumed.next_line().await.unwrap().as_deref(),
        Some("replacement backlog")
    );
}

#[tokio::test]
async fn durable_cursor_reports_lost_rotation_and_copytruncate_as_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    std::fs::write(&path, "initial history\n").unwrap();
    let mut reader = TailReader::open_durable(&path).await.unwrap();
    let saved = reader.cursor().await.unwrap();
    drop(reader);
    std::fs::rename(&path, dir.path().join("app.log.1")).unwrap();
    std::fs::remove_file(dir.path().join("app.log.1")).unwrap();
    std::fs::write(&path, "replacement\n").unwrap();
    let mut resumed = TailReader::restore(&path, saved).await.unwrap();
    assert!(
        resumed
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .contains("file-tail gap")
    );
    assert_eq!(
        resumed.next_line().await.unwrap().as_deref(),
        Some("replacement")
    );
    std::fs::write(&path, "new\n").unwrap();
    assert!(
        resumed
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .contains("file-tail gap")
    );
    assert_eq!(resumed.next_line().await.unwrap().as_deref(), Some("new"));
}

#[tokio::test]
async fn durable_cursor_detects_regrown_rewrite_with_unchanged_initial_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    let prefix = "p".repeat(512);
    std::fs::write(&path, format!("{prefix}old suffix\n")).unwrap();
    let mut reader = TailReader::open_durable(&path).await.unwrap();
    let saved = reader.cursor().await.unwrap();
    let replacement = format!("{prefix}new longer rewritten suffix\n");
    std::fs::write(&path, &replacement).unwrap();
    assert!(
        reader
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .contains("file-tail gap")
    );
    assert_eq!(
        reader.next_line().await.unwrap().unwrap(),
        replacement.trim_end()
    );
    drop(reader);
    let mut resumed = TailReader::restore(&path, saved).await.unwrap();
    assert!(
        resumed
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .contains("file-tail gap")
    );
    assert_eq!(
        resumed.next_line().await.unwrap().unwrap(),
        replacement.trim_end()
    );
}
