use super::*;
#[test]
fn grouped_enqueues_survive_reload_and_rollback_as_one_unit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let state = Arc::new(Mutex::new(SenderState {
        spool_path: path.clone(),
        spool: SpoolState::default(),
        recovery_required: false,
        last_error_code: None,
    }));
    let mut commits = 0;
    let mut bytes_written = 0;
    commit_enqueues(
        &state,
        (0..100)
            .map(|n| ("source".into(), format!("line {n}")))
            .collect(),
        |path, spool| {
            commits += 1;
            bytes_written += serde_json::to_vec(spool)?.len();
            save_spool(path, spool)
        },
    )
    .unwrap();
    assert_eq!(commits, 1, "one durable snapshot covers all 100 enqueues");
    assert_eq!(
        bytes_written,
        std::fs::metadata(&path).unwrap().len() as usize
    );
    let persisted = load_spool(&path);
    assert_eq!(persisted.spool.records.len(), 100);
    assert_eq!(persisted.spool.records[0].line, "line 0");
    assert_eq!(persisted.spool.records[99].sequence, 100);
    let before = serde_json::to_value(&state.lock().unwrap().spool).unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    state.lock().unwrap().spool_path = blocker.join("spool");
    assert!(
        enqueue_batch(
            &state,
            vec![
                ("source".into(), "failed 1".into()),
                ("source".into(), "failed 2".into())
            ]
        )
        .is_err()
    );
    assert_eq!(
        serde_json::to_value(&state.lock().unwrap().spool).unwrap(),
        before
    );
    assert_eq!(load_spool(&path).spool.records.len(), 100);
}

#[test]
fn persistence_owner_commits_ready_producers_before_acknowledging_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let state = Arc::new(Mutex::new(SenderState {
        spool_path: path.clone(),
        spool: SpoolState::default(),
        recovery_required: false,
        last_error_code: None,
    }));
    let (tx, rx) = mpsc::channel(128);
    let mut replies = Vec::new();
    for n in 0..100 {
        let (reply, receive) = oneshot::channel();
        tx.try_send(PersistCommand::Enqueue {
            source_key: "source".into(),
            line: format!("line {n}"),
            reply,
        })
        .unwrap();
        replies.push(receive);
    }
    drop(tx);
    persistence_owner(state, Arc::new(Notify::new()), rx);
    for receive in replies {
        receive.blocking_recv().unwrap().unwrap();
    }
    assert_eq!(load_spool(&path).spool.records.len(), 100);
}

#[test]
fn near_quota_group_writes_one_snapshot_for_one_hundred_producers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let state = Arc::new(Mutex::new(SenderState {
        spool_path: path.clone(),
        spool: SpoolState::default(),
        recovery_required: false,
        last_error_code: None,
    }));
    enqueue_batch(
        &state,
        (0..256)
            .map(|n| (format!("source-{}", n % 4), "x".repeat(16 * 1024)))
            .collect(),
    )
    .unwrap();
    {
        let state = state.lock().unwrap();
        save_spool(&path, &state.spool).unwrap();
    }
    let before_bytes = std::fs::metadata(&path).unwrap().len();
    assert!(before_bytes > 3 * 1024 * 1024);
    let mut commits = 0;
    let mut written = 0;
    commit_enqueues(
        &state,
        (0..100)
            .map(|n| (format!("source-{}", n % 4), format!("new {n}")))
            .collect(),
        |path, spool| {
            commits += 1;
            written += serde_json::to_vec(spool)?.len();
            save_spool(path, spool)
        },
    )
    .unwrap();
    assert_eq!(commits, 1);
    assert!(
        written < 5 * 1024 * 1024,
        "bounded group must not rewrite one full snapshot per record"
    );
    let loaded = load_spool(&path);
    assert_eq!(
        loaded
            .spool
            .records
            .iter()
            .filter(|r| r.line.starts_with("new "))
            .count(),
        100
    );
}

#[test]
fn sequential_enqueues_append_only_new_records_and_replay_after_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let state = Arc::new(Mutex::new(SenderState {
        spool_path: path.clone(),
        spool: SpoolState::default(),
        recovery_required: false,
        last_error_code: None,
    }));
    enqueue_batch(
        &state,
        (0..240)
            .map(|n| (format!("source{}", n % 4), "x".repeat(16000)))
            .collect(),
    )
    .unwrap();
    {
        let state = state.lock().unwrap();
        save_spool(&path, &state.spool).unwrap();
    }
    let snapshot = std::fs::read(&path).unwrap();
    let journal = path.with_file_name("spool.enqueue-journal");
    let before = std::fs::metadata(&journal).unwrap().len();
    for n in 0..100 {
        enqueue(&state, "small", format!("sequential {n}")).unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), snapshot);
    assert!(std::fs::metadata(&journal).unwrap().len() - before < 32000);
    let loaded = load_spool(&path);
    assert_eq!(
        loaded
            .spool
            .records
            .iter()
            .filter(|r| r.line.starts_with("sequential "))
            .count(),
        100
    );
    assert_eq!(
        serde_json::to_value(&loaded.spool).unwrap(),
        serde_json::to_value(&state.lock().unwrap().spool).unwrap()
    );
    // A torn, unacknowledged append is ignored and repaired before the next append.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(b"{torn")
        .unwrap();
    enqueue(&state, "small", "after torn".into()).unwrap();
    assert_eq!(
        load_spool(&path).spool.records.back().unwrap().line,
        "after torn"
    );
}

#[test]
fn corrupt_journal_fails_closed_and_parent_io_errors_do_not_spin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    save_spool(&path, &SpoolState::default()).unwrap();
    std::fs::write(path.with_file_name("spool.enqueue-journal"), b"broken\n").unwrap();
    let loaded = load_spool(&path);
    assert!(loaded.spool.journal_failed);
    assert!(save_spool(&path, &loaded.spool).is_err());
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "file").unwrap();
    let loaded = load_spool(&blocker.join("spool"));
    assert!(loaded.spool.journal_failed);
}

#[test]
fn failed_append_after_sync_rolls_back_disk_and_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let mut state = SenderState {
        spool_path: path.clone(),
        spool: SpoolState::default(),
        recovery_required: false,
        last_error_code: None,
    };
    journal::append(&mut state, vec![("source".into(), "retained".into())]).unwrap();
    let before = serde_json::to_value(&state.spool).unwrap();
    assert!(
        journal::append_with_sync(
            &mut state,
            vec![("source".into(), "failed".into())],
            |file| {
                file.sync_all()?;
                Err(std::io::Error::other("injected failure after sync"))
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&state.spool).unwrap(), before);
    assert_eq!(
        serde_json::to_value(&load_spool(&path).spool).unwrap(),
        before
    );
    journal::append(&mut state, vec![("source".into(), "next".into())]).unwrap();
    assert_eq!(load_spool(&path).spool.records.back().unwrap().sequence, 2);
}

#[test]
fn compaction_snapshot_sequence_prevents_duplicate_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let mut state = SenderState {
        spool_path: path.clone(),
        spool: SpoolState::default(),
        recovery_required: false,
        last_error_code: None,
    };
    journal::append(&mut state, vec![("source".into(), "retained".into())]).unwrap();
    save_spool(&path, &state.spool).unwrap();
    let journal = path.with_file_name("spool.enqueue-journal");
    let old = std::fs::read(&journal).unwrap();
    // A checkpoint may commit before old journal bytes are removed.
    std::fs::write(&journal, old.repeat(8 * 1024 * 1024 / old.len() + 1)).unwrap();
    journal::append(
        &mut state,
        vec![("source".into(), "after compaction".into())],
    )
    .unwrap();
    assert!(std::fs::metadata(&journal).unwrap().len() < 1024);
    let loaded = load_spool(&path);
    assert_eq!(loaded.spool.records.len(), 2);
    assert_eq!(loaded.spool.records[1].sequence, 2);
    assert_eq!(loaded.spool.records[1].line, "after compaction");
}

#[test]
fn replay_preserves_selected_gap_at_multi_source_eviction_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let mut spool = SpoolState::default();
    for n in 0..511 {
        spool.gaps.push_back(SyslogForwardGap {
            source_instance: format!("historical-{n}"),
            source_epoch: 1,
            from_sequence: 1,
            to_sequence: 1,
            idempotency_key: format!("gap-{n}"),
            observed_at: "2026-09-12T12:00:00Z".into(),
            reason_code: "local_retention_quota".into(),
        });
    }
    for n in 0..MAX_SPOOL_RECORDS {
        spool.records.push_back(SyslogForwardRecord {
            source_instance: format!(
                "{}:{}",
                spool.source_instance,
                if n % 2 == 0 {
                    "source-aaaaaaaaaaaaaaaaaaaaaaaa"
                } else {
                    "source-bbbbbbbbbbbbbbbbbbbbbbbb"
                }
            ),
            source_epoch: 1,
            sequence: n as u64 + 1,
            idempotency_key: format!("record-{n}"),
            observed_at: Utc::now().to_rfc3339(),
            line: "x".repeat(1024),
        });
    }
    save_spool(&path, &spool).unwrap();
    let mut state = SenderState {
        spool_path: path.clone(),
        spool,
        recovery_required: false,
        last_error_code: None,
    };
    journal::append(&mut state, vec![("new-source".into(), "y".repeat(4096))]).unwrap();
    assert_eq!(state.spool.gaps.len(), 512);
    assert!(
        state
            .spool
            .gaps
            .back()
            .unwrap()
            .source_instance
            .ends_with("source-aaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert!(state.spool.gap_overflow_intervals > 0);
    let expected = serde_json::to_value(&state.spool).unwrap();
    for _ in 0..16 {
        assert_eq!(
            serde_json::to_value(&load_spool(&path).spool).unwrap(),
            expected
        );
    }
}

#[test]
fn journal_hashes_source_paths_and_replays_legacy_raw_keys_without_identity_drift() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool");
    let journal_path = path.with_file_name("spool.enqueue-journal");
    let raw = "file:/Users/private/production/access.log";
    let mut state = SenderState {
        spool_path: path.clone(),
        spool: SpoolState::default(),
        recovery_required: false,
        last_error_code: None,
    };
    journal::append(&mut state, vec![(raw.into(), "new-format".into())]).unwrap();
    let bytes = std::fs::read_to_string(&journal_path).unwrap();
    assert!(!bytes.contains("/Users/private"));
    assert!(!bytes.contains("access.log"));
    assert!(bytes.contains("sha256_prefix96"));
    assert_eq!(
        serde_json::to_value(&load_spool(&path).spool).unwrap(),
        serde_json::to_value(&state.spool).unwrap()
    );
    let source_instance = state.spool.records[0].source_instance.clone();
    // Emulate a journal from before the typed canonical-source format.
    let legacy = serde_json::json!({"sequence":1,"at":"2026-09-12T12:00:00Z","records":[[raw,"legacy-format"]]});
    std::fs::write(&journal_path, format!("{legacy}\n")).unwrap();
    let loaded = load_spool(&path);
    assert_eq!(loaded.spool.records[0].source_instance, source_instance);
    assert_eq!(loaded.spool.records[0].line, "legacy-format");
    state.spool = loaded.spool;
    journal::append(&mut state, vec![(raw.into(), "after-upgrade".into())]).unwrap();
    let loaded = load_spool(&path);
    assert_eq!(loaded.spool.records.len(), 2);
    assert_eq!(loaded.spool.records[1].source_instance, source_instance);
    assert_eq!(loaded.spool.records[1].sequence, 2);
}
