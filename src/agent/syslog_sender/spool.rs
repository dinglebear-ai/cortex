//! Bounded durable spool retention and source round-robin helpers.

use super::*;

pub(super) fn evict_source_to_quota(spool: &mut SpoolState, source_key: &str) {
    let cutoff = Utc::now() - chrono::Duration::seconds(MAX_SPOOL_AGE_SECS);
    let mut from = None;
    let mut to = None;
    while let Some(index) = spool.records.iter().position(|record| {
        source_key_of(&record.source_instance).is_some_and(|key| key == source_key)
            && chrono::DateTime::parse_from_rfc3339(&record.observed_at)
                .map(|at| at.with_timezone(&Utc) < cutoff)
                .unwrap_or(true)
    }) {
        pop_evicted_at(spool, index, &mut from, &mut to);
    }
    while spool
        .records
        .iter()
        .filter(|record| {
            source_key_of(&record.source_instance).is_some_and(|key| key == source_key)
        })
        .count()
        > MAX_SOURCE_SPOOL_RECORDS
        || spool
            .records
            .iter()
            .filter(|record| {
                source_key_of(&record.source_instance).is_some_and(|key| key == source_key)
            })
            .map(|record| record.line.len())
            .sum::<usize>()
            > MAX_SOURCE_SPOOL_BYTES
    {
        let index = spool
            .records
            .iter()
            .position(|record| {
                source_key_of(&record.source_instance).is_some_and(|key| key == source_key)
            })
            .expect("source quota implies source record");
        pop_evicted_at(spool, index, &mut from, &mut to);
    }
    if let (Some(from_sequence), Some(to_sequence)) = (from, to) {
        let source = format!("{}:{source_key}", spool.source_instance);
        spool.gaps.push_back(SyslogForwardGap {
            source_instance: source.clone(),
            source_epoch: spool.source_epoch,
            from_sequence,
            to_sequence,
            idempotency_key: delivery_key(&source, spool.source_epoch, to_sequence, "gap"),
            observed_at: now(),
            reason_code: "local_retention_quota".into(),
        });
        while spool.gaps.len() > 256 {
            spool.gaps.pop_front();
        }
    }
}
fn pop_evicted_at(
    spool: &mut SpoolState,
    index: usize,
    from: &mut Option<u64>,
    to: &mut Option<u64>,
) {
    if let Some(record) = spool.records.remove(index) {
        *from = Some(from.unwrap_or(record.sequence));
        *to = Some(record.sequence);
        spool.evicted_records = spool.evicted_records.saturating_add(1);
    }
}
pub(super) fn evict_aggregate_to_quota(spool: &mut SpoolState) {
    let mut windows: HashMap<String, (u64, u64)> = HashMap::new();
    while spool.records.len() > MAX_SPOOL_RECORDS
        || spool
            .records
            .iter()
            .map(|record| record.line.len())
            .sum::<usize>()
            > MAX_SPOOL_BYTES
    {
        let Some(record) = spool.records.pop_front() else {
            break;
        };
        spool.evicted_records = spool.evicted_records.saturating_add(1);
        windows
            .entry(record.source_instance)
            .and_modify(|window| window.1 = record.sequence)
            .or_insert((record.sequence, record.sequence));
    }
    for (source_instance, (from_sequence, to_sequence)) in windows {
        spool.gaps.push_back(SyslogForwardGap {
            idempotency_key: delivery_key(&source_instance, spool.source_epoch, to_sequence, "gap"),
            source_instance,
            source_epoch: spool.source_epoch,
            from_sequence,
            to_sequence,
            observed_at: now(),
            reason_code: "aggregate_retention_quota".into(),
        });
    }
    while spool.gaps.len() > 256 {
        spool.gaps.pop_front();
    }
}
pub(super) fn next_source_key(spool: &SpoolState) -> Option<String> {
    let mut sources = BTreeSet::new();
    for record in &spool.records {
        if let Some(source) = source_key_of(&record.source_instance) {
            sources.insert(source.to_string());
        }
    }
    for gap in &spool.gaps {
        if let Some(source) = source_key_of(&gap.source_instance) {
            sources.insert(source.to_string());
        }
    }
    let mut sources = sources.into_iter().collect::<Vec<_>>();
    if sources.is_empty() {
        return None;
    }
    if let Some(last) = spool.last_dispatched_source.as_ref()
        && let Some(position) = sources.iter().position(|source| source == last)
    {
        let len = sources.len();
        sources.rotate_left((position + 1) % len);
    }
    sources.into_iter().next()
}
pub(super) fn source_key_of(source_instance: &str) -> Option<&str> {
    let (_, key) = source_instance.rsplit_once(':')?;
    let digest = key.strip_prefix("source-")?;
    (digest.len() == 24 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(key)
}
