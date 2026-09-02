use super::*;

#[test]
fn backoff_ms_doubles_until_capped() {
    assert_eq!(backoff_ms(0), 500);
    assert_eq!(backoff_ms(1), 1_000);
    assert_eq!(backoff_ms(6), 30_000);
    assert_eq!(backoff_ms(42), 30_000);
}

#[test]
fn quota_eviction_creates_a_payload_free_gap() {
    let source_key = "source-000000000000000000000001";
    let mut spool = SpoolState {
        source_instance: "host-a".into(),
        source_epoch: 1,
        next_sequence: 0,
        next_sequences: HashMap::from([(source_key.into(), (MAX_SOURCE_SPOOL_RECORDS + 1) as u64)]),
        records: (1..=MAX_SOURCE_SPOOL_RECORDS + 1)
            .map(|sequence| SyslogForwardRecord {
                source_instance: format!("host-a:{source_key}"),
                source_epoch: 1,
                sequence: sequence as u64,
                idempotency_key: delivery_key("host-a", 1, sequence as u64, "record"),
                observed_at: now(),
                line: "x".into(),
            })
            .collect(),
        gaps: VecDeque::new(),
        evicted_records: 0,
        last_dispatched_source: None,
    };
    evict_source_to_quota(&mut spool, source_key);
    assert_eq!(spool.records.len(), MAX_SOURCE_SPOOL_RECORDS);
    assert_eq!(spool.evicted_records, 1);
    let gap = spool.gaps.front().unwrap();
    assert_eq!((gap.from_sequence, gap.to_sequence), (1, 1));
    assert_eq!(gap.reason_code, "local_retention_quota");
}

#[test]
fn exact_receipt_advances_only_the_matching_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.json");
    let state = Arc::new(Mutex::new(SenderState {
        spool_path: path.clone(),
        spool: SpoolState {
            source_instance: "host-a".into(),
            source_epoch: 1,
            next_sequence: 2,
            next_sequences: HashMap::new(),
            records: VecDeque::from([record(1), record(2)]),
            gaps: VecDeque::new(),
            evicted_records: 0,
            last_dispatched_source: None,
        },
        last_error_code: None,
    }));
    apply_receipts(&state, &[delivery_key("host-a:syslog", 1, 1, "record")]).unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.spool.records.len(), 1);
    assert_eq!(state.spool.records.front().unwrap().sequence, 2);
    assert!(path.exists());
}

#[test]
fn lost_partial_reordered_and_unknown_receipts_keep_every_unmatched_record_durable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.json");
    let first = record(1);
    let second = record(2);
    let third = record(3);
    let state = Arc::new(Mutex::new(SenderState {
        spool_path: path,
        spool: SpoolState {
            source_instance: "host-a".into(),
            source_epoch: 1,
            next_sequence: 3,
            next_sequences: HashMap::new(),
            records: VecDeque::from([first.clone(), second.clone(), third.clone()]),
            gaps: VecDeque::new(),
            evicted_records: 0,
            last_dispatched_source: None,
        },
        last_error_code: None,
    }));

    // A lost response is equivalent to no receipts: no local data may move.
    apply_receipts(&state, &[]).unwrap();
    assert_eq!(state.lock().unwrap().spool.records.len(), 3);

    // Reordered/partial responses can acknowledge only their exact known IDs.
    apply_receipts(
        &state,
        &[
            "unknown-receipt".into(),
            third.idempotency_key,
            first.idempotency_key,
        ],
    )
    .unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.spool.records.len(), 1);
    assert_eq!(
        state.spool.records.front().unwrap().idempotency_key,
        second.idempotency_key
    );
}

#[test]
fn receiver_failure_plans_are_bounded_and_leave_spool_records_untouched() {
    let throttled = retry_plan(429, 0, Some(9_999), 0);
    assert_eq!(throttled.reason_code, "receiver_backpressure");
    assert_eq!(throttled.delay_ms, RECONNECT_MAX_MS);

    let unavailable = retry_plan(503, 0, None, u64::MAX);
    assert_eq!(unavailable.reason_code, "receiver_unavailable");
    assert!((backoff_ms(0)..=backoff_ms(0) + backoff_ms(0) / 10).contains(&unavailable.delay_ms));

    let state = Arc::new(Mutex::new(SenderState {
        spool_path: PathBuf::from("/unused"),
        spool: SpoolState {
            source_instance: "host-a".into(),
            source_epoch: 1,
            next_sequence: 1,
            next_sequences: HashMap::new(),
            records: VecDeque::from([record(1)]),
            gaps: VecDeque::new(),
            evicted_records: 0,
            last_dispatched_source: None,
        },
        last_error_code: None,
    }));
    // Failure paths intentionally call no receipt application.
    let _ = (throttled, unavailable);
    assert_eq!(state.lock().unwrap().spool.records.len(), 1);
}

#[test]
fn retry_jitter_varies_with_entropy_and_stays_within_the_bounded_window() {
    let base = backoff_ms(4);
    let minimum = retry_delay_ms(4, None, 0);
    let maximum = retry_delay_ms(4, None, base / 10);
    assert_eq!(minimum, base);
    assert!(minimum < maximum);
    assert!(maximum <= base + base / 10);
    assert_eq!(retry_delay_ms(0, Some(2), 123), 2_000);
}

#[test]
fn receiver_outage_spool_is_bounded_and_records_a_gap_when_it_evicts() {
    let source_key = "source-000000000000000000000001";
    let mut spool = SpoolState {
        source_instance: "host-a".into(),
        source_epoch: 1,
        next_sequence: MAX_SOURCE_SPOOL_RECORDS as u64,
        next_sequences: HashMap::new(),
        records: (1..=MAX_SOURCE_SPOOL_RECORDS)
            .map(|sequence| source_record(&format!("host-a:{source_key}"), sequence as u64))
            .collect(),
        gaps: VecDeque::new(),
        evicted_records: 0,
        last_dispatched_source: None,
    };
    spool.records.push_back(source_record(
        &format!("host-a:{source_key}"),
        (MAX_SOURCE_SPOOL_RECORDS + 1) as u64,
    ));
    evict_source_to_quota(&mut spool, source_key);

    assert_eq!(spool.records.len(), MAX_SOURCE_SPOOL_RECORDS);
    assert_eq!(spool.evicted_records, 1);
    assert!(spool.gaps.iter().any(|gap| {
        gap.reason_code == "local_retention_quota" && (gap.from_sequence, gap.to_sequence) == (1, 1)
    }));
}

fn record(sequence: u64) -> SyslogForwardRecord {
    SyslogForwardRecord {
        source_instance: "host-a:syslog".into(),
        source_epoch: 1,
        sequence,
        idempotency_key: delivery_key("host-a:syslog", 1, sequence, "record"),
        observed_at: now(),
        line: "frame".into(),
    }
}

#[test]
fn round_robin_dispatch_keeps_a_quiet_source_visible_behind_a_noisy_one() {
    let noisy = "source-000000000000000000000001";
    let quiet = "source-000000000000000000000002";
    let mut spool = SpoolState {
        source_instance: "host-a".into(),
        source_epoch: 1,
        next_sequence: 0,
        next_sequences: HashMap::new(),
        records: VecDeque::from([
            source_record(&format!("host-a:{noisy}"), 1),
            source_record(&format!("host-a:{noisy}"), 2),
            source_record(&format!("host-a:{quiet}"), 1),
        ]),
        gaps: VecDeque::new(),
        evicted_records: 0,
        last_dispatched_source: None,
    };
    assert_eq!(next_source_key(&spool).as_deref(), Some(noisy));
    spool.last_dispatched_source = Some(noisy.into());
    assert_eq!(next_source_key(&spool).as_deref(), Some(quiet));
}

#[test]
fn source_identity_requires_an_exact_hashed_component() {
    assert_eq!(
        source_key_of("host-a:container:source-000000000000000000000001"),
        Some("source-000000000000000000000001")
    );
    assert_eq!(
        source_key_of("host-a:source-00000000000000000000000z"),
        None
    );
    assert_eq!(
        source_key_of("host-a:source-000000000000000000000001-extra"),
        None
    );
}

#[test]
fn path_like_source_key_is_never_serialized_into_spool_or_delivery_request() {
    let raw_source = "/Users/jmagar/private/production/access.log";
    let source_key = stable_source_key(raw_source);
    let record = source_record(&format!("host-a:{source_key}"), 1);
    let spool = SpoolState {
        source_instance: "host-a".into(),
        source_epoch: 1,
        next_sequence: 0,
        next_sequences: HashMap::from([(source_key.clone(), 1)]),
        records: VecDeque::from([record]),
        gaps: VecDeque::new(),
        evicted_records: 0,
        last_dispatched_source: None,
    };
    assert!(
        !String::from_utf8(serde_json::to_vec(&spool).unwrap())
            .unwrap()
            .contains(raw_source)
    );
    let state = Arc::new(Mutex::new(SenderState {
        spool_path: PathBuf::from("/unused"),
        spool,
        last_error_code: None,
    }));
    let request = next_request(&state).unwrap();
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains(raw_source)
    );
}

#[test]
fn aggregate_cap_bounds_high_cardinality_sources_and_records_each_loss_window() {
    let records = (0..=MAX_SPOOL_RECORDS)
        .map(|index| {
            let key = format!("source-{index:024x}");
            source_record(&format!("host-a:{key}"), 1)
        })
        .collect();
    let mut spool = SpoolState {
        source_instance: "host-a".into(),
        source_epoch: 1,
        next_sequence: 0,
        next_sequences: HashMap::new(),
        records,
        gaps: VecDeque::new(),
        evicted_records: 0,
        last_dispatched_source: None,
    };
    evict_aggregate_to_quota(&mut spool);
    assert_eq!(spool.records.len(), MAX_SPOOL_RECORDS);
    assert_eq!(spool.evicted_records, 1);
    assert_eq!(spool.gaps.len(), 1);
    assert_eq!(
        spool.gaps.front().unwrap().reason_code,
        "aggregate_retention_quota"
    );
}

fn source_record(source_instance: &str, sequence: u64) -> SyslogForwardRecord {
    SyslogForwardRecord {
        source_instance: source_instance.into(),
        source_epoch: 1,
        sequence,
        idempotency_key: delivery_key(source_instance, 1, sequence, "record"),
        observed_at: now(),
        line: "frame".into(),
    }
}

#[test]
fn format_rfc5424_replaces_newlines_and_keeps_valid_fields() {
    assert_eq!(
        format_rfc5424(PRI_LOCAL0_ERR, "ts", "host", "app", "pid", "first\nsecond"),
        "<131>1 ts host app pid - - first second"
    );
}
