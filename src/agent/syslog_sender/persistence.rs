//! Dedicated durable-spool owner and transactional mutations.

use super::*;

#[cfg(test)]
pub(super) fn enqueue(
    state: &Arc<Mutex<SenderState>>,
    source_key: &str,
    line: String,
) -> Result<()> {
    enqueue_batch(state, vec![(source_key.to_owned(), line)])
}

fn enqueue_batch(state: &Arc<Mutex<SenderState>>, records: Vec<(String, String)>) -> Result<()> {
    let mut state = state.lock().expect("syslog sender state poisoned");
    journal::append(&mut state, records)
}

#[cfg(test)]
fn commit_enqueues(
    state: &Arc<Mutex<SenderState>>,
    records: Vec<(String, String)>,
    mut persist: impl FnMut(&std::path::Path, &SpoolState) -> Result<()>,
) -> Result<()> {
    let mut state = state.lock().expect("syslog sender state poisoned");
    let previous_spool = state.spool.clone();
    for (source_key, line) in records {
        enqueue_in_memory(&mut state.spool, &source_key, line);
    }
    if let Err(error) = persist(&state.spool_path, &state.spool) {
        state.spool = previous_spool;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
fn enqueue_in_memory(spool: &mut SpoolState, source_key: &str, line: String) {
    enqueue_at(spool, source_key, line, Utc::now());
}

pub(super) fn enqueue_at(
    spool: &mut SpoolState,
    source_key: &str,
    line: String,
    at: chrono::DateTime<Utc>,
) {
    let source_key = stable_source_key(source_key);
    let sequence = {
        let legacy_next = spool.next_sequence;
        let next = spool
            .next_sequences
            .entry(source_key.clone())
            .or_insert(legacy_next);
        *next = next.saturating_add(1);
        *next
    };
    let source_instance = format!("{}:{source_key}", spool.source_instance);
    let epoch = spool.source_epoch;
    if line.len() > MAX_FORWARD_RECORD_BYTES {
        push_gap(
            spool,
            SyslogForwardGap {
                source_instance: source_instance.clone(),
                source_epoch: epoch,
                from_sequence: sequence,
                to_sequence: sequence,
                idempotency_key: delivery_key(&source_instance, epoch, sequence, "gap"),
                observed_at: at.to_rfc3339(),
                reason_code: "record_too_large".into(),
            },
        );
        spool.evicted_records = spool.evicted_records.saturating_add(1);
    } else {
        spool.records.push_back(SyslogForwardRecord {
            source_instance: source_instance.clone(),
            source_epoch: epoch,
            sequence,
            idempotency_key: delivery_key(&source_instance, epoch, sequence, "record"),
            observed_at: at.to_rfc3339(),
            line,
        });
    }
    spool::evict_source_at(spool, &source_key, at);
    spool::evict_aggregate_at(spool, at);
}

pub(super) fn persistence_owner(
    state: Arc<Mutex<SenderState>>,
    notify: Arc<Notify>,
    mut commands: mpsc::Receiver<PersistCommand>,
) {
    let mut pending = None;
    while let Some(command) = pending.take().or_else(|| commands.blocking_recv()) {
        match command {
            PersistCommand::Enqueue {
                source_key,
                line,
                reply,
            } => {
                // Commit a bounded run of ready producers together. Never ack
                // a member until the entire snapshot and quota gaps are synced.
                let mut records = vec![(source_key, line)];
                let mut replies = vec![reply];
                while records.len() < PERSIST_COMMAND_CAPACITY {
                    match commands.try_recv() {
                        Ok(PersistCommand::Enqueue {
                            source_key,
                            line,
                            reply,
                        }) => {
                            records.push((source_key, line));
                            replies.push(reply);
                        }
                        Ok(command) => {
                            pending = Some(command);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                let result = enqueue_batch(&state, records);
                if result.is_ok() {
                    notify.notify_one();
                }
                let error = result.err().map(|error| format!("{error:#}"));
                for reply in replies {
                    if reply.is_closed()
                        && let Some(error) = &error
                    {
                        tracing::error!(
                            reason_code = "local_spool_persist_failed",
                            error,
                            "syslog frame could not be retained"
                        );
                    }
                    let _ = reply.send(match &error {
                        Some(error) => Err(anyhow!(error.clone())),
                        None => Ok(()),
                    });
                }
            }
            PersistCommand::NextRequest { reply } => {
                let _ = reply.send(next_request(&state));
            }
            PersistCommand::ApplyReceipts {
                request,
                receipts,
                reply,
            } => {
                let _ = reply.send(apply_receipts(&state, &request, &receipts));
            }
        }
    }
}
pub(super) fn next_request(
    state: &Arc<Mutex<SenderState>>,
) -> Result<Option<SyslogForwardRequest>> {
    let mut state = state.lock().expect("syslog sender state poisoned");
    let Some(source_key) = next_source_key(&state.spool) else {
        return Ok(None);
    };
    let mut bytes = 0usize;
    let records = state
        .spool
        .records
        .iter()
        .filter(|record| {
            source_key_of(&record.source_instance).is_some_and(|key| key == source_key)
        })
        .take_while(|record| {
            let include = bytes == 0 || bytes.saturating_add(record.line.len()) <= MAX_BATCH_BYTES;
            if include {
                bytes = bytes.saturating_add(record.line.len());
            }
            include
        })
        .take(MAX_BATCH_RECORDS)
        .cloned()
        .collect();
    let gaps: Vec<SyslogForwardGap> = state
        .spool
        .gaps
        .iter()
        .filter(|gap| source_key_of(&gap.source_instance).is_some_and(|key| key == source_key))
        .take(50)
        .cloned()
        .collect();
    let previous_spool = state.spool.clone();
    state.spool.last_dispatched_source = Some(source_key);
    state
        .spool
        .dispatched_gap_keys
        .extend(gaps.iter().map(|gap| gap.idempotency_key.clone()));
    if let Err(error) = save_spool(&state.spool_path, &state.spool) {
        state.spool = previous_spool;
        return Err(error);
    }
    Ok(Some(SyslogForwardRequest { records, gaps }))
}

pub(super) fn apply_receipts(
    state: &Arc<Mutex<SenderState>>,
    request: &SyslogForwardRequest,
    receipts: &[String],
) -> Result<()> {
    let outbound_keys: HashSet<&str> = request
        .records
        .iter()
        .map(|record| record.idempotency_key.as_str())
        .chain(request.gaps.iter().map(|gap| gap.idempotency_key.as_str()))
        .collect();
    let keys: HashSet<&str> = receipts.iter().map(String::as_str).collect();
    if receipts.len() != outbound_keys.len()
        || keys.len() != receipts.len()
        || keys != outbound_keys
    {
        return Err(anyhow::Error::new(InvalidReceiptSet));
    }
    let mut state = state.lock().expect("syslog sender state poisoned");
    let previous_spool = state.spool.clone();
    state
        .spool
        .records
        .retain(|record| !keys.contains(record.idempotency_key.as_str()));
    state
        .spool
        .gaps
        .retain(|gap| !keys.contains(gap.idempotency_key.as_str()));
    state
        .spool
        .dispatched_gap_keys
        .retain(|key| !keys.contains(key.as_str()));
    if let Err(error) = save_spool(&state.spool_path, &state.spool) {
        state.spool = previous_spool;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
