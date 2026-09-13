use super::*;
#[tokio::test]
async fn checkpoint_resumes_same_file_but_not_a_larger_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    std::fs::write(&path, "old\n").unwrap();
    let mut source = FileTailSource::from_add(
        crate::filetail::models::FileTailAddRequest {
            id: "test".into(),
            path: path.to_string_lossy().into_owned(),
            tag: "test".into(),
            host: None,
            facility: None,
            severity: None,
            start_at_end: Some(false),
        },
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let opened = open_tail_file(&source, true).await.unwrap();
    source.checkpoint_dev = Some(opened.identity.dev);
    source.checkpoint_ino = Some(opened.identity.ino);
    source.checkpoint_offset = Some(4);
    drop(opened);
    assert_eq!(open_tail_file(&source, true).await.unwrap().position, 4);
    std::fs::rename(&path, dir.path().join("old")).unwrap();
    std::fs::write(&path, "replacement bigger than checkpoint\n").unwrap();
    let replaced = open_tail_file(&source, true).await.unwrap();
    assert_eq!(replaced.position, 0);
    assert_ne!(
        (replaced.identity.dev, replaced.identity.ino),
        (
            source.checkpoint_dev.unwrap(),
            source.checkpoint_ino.unwrap()
        )
    );
}
