use super::*;
use serial_test::serial;

fn budget() -> DiscoveryBudget {
    DiscoveryBudget {
        max_entries: 2,
        max_path_bytes: 4096,
        deadline: Duration::from_millis(1),
        cancel: None,
    }
}

fn release() {
    *SNAPSHOT_TEST_RELEASE.0.lock().unwrap() = true;
    SNAPSHOT_TEST_RELEASE.1.notify_all();
}

fn poll(state: &DiscoveryContinuation, root: &Path, cursor: Option<&Path>) -> DiscoveryResult {
    let limit = Instant::now() + Duration::from_secs(3);
    loop {
        let result = state.discover(root.to_owned(), cursor, budget()).unwrap();
        if !result.files.is_empty() || !result.capped {
            return result;
        }
        assert!(Instant::now() < limit, "discovery did not finish");
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn consume(state: &DiscoveryContinuation, root: &Path, page: &DiscoveryResult) -> IndexResult {
    for file in &page.files {
        state.source_finished(file);
    }
    let mut result = IndexResult {
        discovery_cap_hits: 1,
        discovery_deferred_roots: 1,
        next_discovery_cursors: [(root.to_owned(), root.to_owned())].into(),
        ..Default::default()
    };
    state.finish_pass(&mut result);
    result
}

#[test]
#[serial]
fn unfinished_enumeration_retains_worker_and_emits_only_ordered_pages() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["z.jsonl", "pause.jsonl", "a.jsonl"] {
        fs::write(dir.path().join(name), "{}\n").unwrap();
    }
    let _hold = SnapshotWorkerGuard::for_file("pause.jsonl");
    let state = DiscoveryContinuation::default();
    for _ in 0..20 {
        let result = state
            .discover(dir.path().to_owned(), None, budget())
            .unwrap();
        assert!(result.capped);
        assert!(result.files.is_empty());
        assert!(result.next_cursor.is_none());
        assert_eq!(state.0.pending.lock().unwrap().len(), 1);
    }
    release();
    let first = poll(&state, dir.path(), None);
    assert_eq!(
        first.files,
        vec![dir.path().join("a.jsonl"), dir.path().join("pause.jsonl")]
    );
    assert!(first.capped);
    assert!(
        first.next_cursor.is_none(),
        "page cursor waits for source acknowledgements"
    );
    let consumed = consume(&state, dir.path(), &first);
    assert_eq!(
        consumed.next_discovery_cursors.get(dir.path()),
        Some(&dir.path().join("pause.jsonl"))
    );
    let second = poll(
        &state,
        dir.path(),
        consumed
            .next_discovery_cursors
            .get(dir.path())
            .map(PathBuf::as_path),
    );
    assert_eq!(second.files, vec![dir.path().join("z.jsonl")]);
    assert!(second.capped, "final page still owns unfinished sources");
    assert_eq!(
        consume(&state, dir.path(), &second).discovery_deferred_roots,
        0
    );
    assert!(!state.is_pending());
}

#[test]
#[serial]
fn one_blocked_root_allows_healthy_root_and_two_workers_bound_capacity() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["slow-one", "slow-two", "healthy", "capacity-deferred"] {
        fs::create_dir(dir.path().join(name)).unwrap();
        fs::write(dir.path().join(name).join("session.jsonl"), "{}\n").unwrap();
    }
    let _hold = SnapshotWorkerGuard::for_files(["slow-one", "slow-two"]);
    let state = DiscoveryContinuation::default();
    state
        .discover(dir.path().join("slow-one"), None, budget())
        .unwrap();
    let healthy = poll(&state, &dir.path().join("healthy"), None);
    assert_eq!(healthy.files.len(), 1);
    state
        .discover(dir.path().join("slow-two"), None, budget())
        .unwrap();
    for _ in 0..20 {
        state
            .discover(dir.path().join("slow-one"), None, budget())
            .unwrap();
    }
    assert_eq!(DISCOVERY_WORKERS_IN_FLIGHT.load(Ordering::Acquire), 2);
    let deferred = state
        .discover(dir.path().join("capacity-deferred"), None, budget())
        .unwrap();
    assert!(deferred.capped);
    assert_eq!(state.0.pending.lock().unwrap().len(), 2);
    release();
    assert_eq!(
        poll(&state, &dir.path().join("slow-one"), None).files.len(),
        1
    );
    assert_eq!(
        poll(&state, &dir.path().join("slow-two"), None).files.len(),
        1
    );
}

#[test]
#[serial]
fn dropped_generation_cancels_worker_without_releasing_its_slot_early() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("pause.jsonl"), "{}\n").unwrap();
    let _hold = SnapshotWorkerGuard::for_file("pause.jsonl");
    let mut state = DiscoveryContinuation::default();
    state
        .discover(dir.path().to_owned(), None, budget())
        .unwrap();
    let turn = state
        .0
        .pending
        .lock()
        .unwrap()
        .get(dir.path())
        .unwrap()
        .turn
        .clone();
    let old_generation = state.clone();
    state.clear();
    assert!(!old_generation.is_pending());
    assert!(old_generation.0.cancelled.load(Ordering::Acquire));
    assert!(turn.state.lock().unwrap().1);
    assert!(!state.is_pending());
    _hold.finish();
    assert_eq!(DISCOVERY_WORKERS_IN_FLIGHT.load(Ordering::Acquire), 0);
    assert_eq!(poll(&state, dir.path(), None).files.len(), 1);
}

#[test]
#[serial]
fn changed_root_invalidates_finished_page_without_cursor_advance() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a.jsonl"), "{}\n").unwrap();
    let hold = SnapshotWorkerGuard::for_file("a.jsonl");
    let state = DiscoveryContinuation::default();
    state
        .discover(
            root.clone(),
            None,
            DiscoveryBudget {
                deadline: Duration::from_secs(2),
                ..budget()
            },
        )
        .unwrap();
    hold.finish();
    fs::rename(&root, parent.path().join("old-root")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("b.jsonl"), "{}\n").unwrap();
    let stale = state.discover(root.clone(), None, budget()).unwrap();
    assert!(stale.capped);
    assert!(stale.files.is_empty());
    assert!(stale.next_cursor.is_none());
    assert_eq!(poll(&state, &root, None).files, vec![root.join("b.jsonl")]);
}

#[test]
fn cooperative_deadline_parks_until_next_turn_and_cancellation_wakes_it() {
    let turn = std::sync::Arc::new(DiscoveryTurn::new(Duration::from_secs(60)));
    turn.state.lock().unwrap().0 = Instant::now() - Duration::from_secs(61);
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_turn = turn.clone();
    let worker = std::thread::spawn(move || sender.send(worker_turn.checkpoint()).unwrap());
    assert!(receiver.recv_timeout(Duration::from_millis(10)).is_err());
    turn.resume();
    assert!(!receiver.recv_timeout(Duration::from_secs(1)).unwrap());
    worker.join().unwrap();
    turn.state.lock().unwrap().0 = Instant::now() - Duration::from_secs(61);
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_turn = turn.clone();
    let worker = std::thread::spawn(move || sender.send(worker_turn.checkpoint()).unwrap());
    assert!(receiver.recv_timeout(Duration::from_millis(10)).is_err());
    turn.cancel();
    assert!(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
    worker.join().unwrap();
}

#[test]
fn nested_discovery_shares_count_and_path_byte_allowance() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a/a.jsonl", "a/z.jsonl", "b/a.jsonl", "b/z.jsonl"] {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "{}\n").unwrap();
    }
    let max_path_bytes =
        dir.path().join("a").as_os_str().len() + dir.path().join("a/a.jsonl").as_os_str().len();
    let budget = DiscoveryBudget {
        max_entries: 2,
        max_path_bytes,
        deadline: Duration::from_secs(2),
        cancel: None,
    };
    let mut result = DiscoveryResult::default();
    let (mut entries, mut bytes) = (0, 0);
    collect_supported_files_bounded(
        dir.path(),
        &mut result,
        &mut entries,
        &mut bytes,
        None,
        &budget,
        Instant::now(),
    );
    assert!(result.capped);
    assert_eq!(result.files, vec![dir.path().join("a/a.jsonl")]);
    assert_eq!(entries, 2);
    assert_eq!(bytes, max_path_bytes);
}

#[test]
#[serial]
fn completed_roots_do_not_restart_while_other_roots_are_pending() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["first", "second"] {
        fs::create_dir(dir.path().join(name)).unwrap();
        fs::write(dir.path().join(name).join("a.jsonl"), "{}\n").unwrap();
    }
    let mut state = DiscoveryContinuation::default();
    let first = poll(&state, &dir.path().join("first"), None);
    consume(&state, &dir.path().join("first"), &first);
    state
        .discover(dir.path().join("second"), None, budget())
        .unwrap();
    assert!(
        state
            .discover(dir.path().join("first"), None, budget())
            .unwrap()
            .files
            .is_empty()
    );
    let second = poll(&state, &dir.path().join("second"), None);
    consume(&state, &dir.path().join("second"), &second);
    assert!(!state.is_pending());
    state.clear();
    assert_eq!(poll(&state, &dir.path().join("first"), None).files.len(), 1);
}

#[test]
#[serial]
fn vanished_root_cancels_parked_worker_without_releasing_blocked_admission() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("vanished");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("pause.jsonl"), "{}\n").unwrap();
    let hold = SnapshotWorkerGuard::for_file("pause.jsonl");
    let state = DiscoveryContinuation::default();
    state.discover(root.clone(), None, budget()).unwrap();
    let turn = state
        .0
        .pending
        .lock()
        .unwrap()
        .get(&root)
        .unwrap()
        .turn
        .clone();
    fs::remove_dir_all(&root).unwrap();
    state.forget_root(&root);
    assert!(turn.state.lock().unwrap().1);
    assert!(!state.is_pending());
    hold.finish();
    assert_eq!(DISCOVERY_WORKERS_IN_FLIGHT.load(Ordering::Acquire), 0);
}

#[test]
#[serial]
#[cfg(unix)]
fn terminal_path_error_does_not_pin_page_with_byte_deferred_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".claude/projects");
    fs::create_dir_all(&root).unwrap();
    let invalidated = root.join("a.jsonl");
    let deferred = root.join("b.jsonl");
    fs::write(&invalidated, "{}\n").unwrap();
    let rows = (0..6)
        .map(|row| {
            serde_json::json!({"sessionId":"ready-terminal",
            "content": format!("deferred after terminal row {row}")})
            .to_string()
                + "\n"
        })
        .collect::<String>();
    fs::write(&deferred, rows).unwrap();
    let state = DiscoveryContinuation::default();
    assert_eq!(poll(&state, &root, None).files.len(), 2);
    fs::remove_file(&invalidated).unwrap();
    // Keep root metadata unchanged between ready-page validation and the path
    // error by installing a ready page after replacing the discovered locator.
    std::os::unix::fs::symlink(&deferred, &invalidated).unwrap();
    state
        .0
        .ready
        .lock()
        .unwrap()
        .get_mut(&root)
        .unwrap()
        .identity = discovery::directory_identity(&root);
    let storage = StorageConfig::for_test(dir.path().join("terminal.db"));
    let pool = crate::db::init_pool(&storage).unwrap();
    let mut options = IndexOptions {
        root_override: Some(root.clone()),
        scan_budget: Some(ScanBudget {
            per_source_max_bytes: 128,
            scan_max_bytes: 128,
            per_source_deadline: Duration::from_secs(1),
        }),
        ..Default::default()
    };
    let first = index_roots_with_continuation(&pool, options.clone(), None, Some(&state)).unwrap();
    assert_eq!(first.skipped_symlinks, 1);
    assert!(first.source_budget_cap_hits > 0);
    assert_eq!(
        state.0.ready.lock().unwrap().get(&root).unwrap().page.files,
        vec![deferred]
    );
    options.start_after = first.next_scan_cursor;
    options.discovery_start_after = first.next_discovery_cursors;
    let mut completed = false;
    for _ in 0..20 {
        let result =
            index_roots_with_continuation(&pool, options.clone(), None, Some(&state)).unwrap();
        options.start_after = result.next_scan_cursor;
        options.discovery_start_after = result.next_discovery_cursors;
        if result.discovery_deferred_roots == 0 {
            completed = true;
            break;
        }
    }
    assert!(completed);
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 6);
}
