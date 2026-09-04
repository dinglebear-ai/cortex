//! Bounded durable spool retention and source round-robin helpers.

use super::*;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

pub(super) struct LoadedSpool {
    pub(super) spool: SpoolState,
    pub(super) spool_path: PathBuf,
    pub(super) error_code: Option<&'static str>,
}

pub(super) fn load_spool(path: &Path) -> LoadedSpool {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(spool) => LoadedSpool {
                spool,
                spool_path: path.to_path_buf(),
                error_code: None,
            },
            Err(error) => recover_unreadable_spool(path, error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LoadedSpool {
            spool: SpoolState::default(),
            spool_path: path.to_path_buf(),
            error_code: None,
        },
        Err(error) => recover_unreadable_spool(path, error),
    }
}

fn recover_unreadable_spool(path: &Path, error: impl std::fmt::Display) -> LoadedSpool {
    let recovery_path = path.with_extension(format!("recovery-{}", random_u64()));
    tracing::error!(
        spool = %path.display(),
        recovery_spool = %recovery_path.display(),
        error = %error,
        "syslog spool is unreadable; retaining original and starting a separate recovery spool"
    );
    LoadedSpool {
        spool: SpoolState::default(),
        spool_path: recovery_path,
        error_code: Some("spool_recovery_required"),
    }
}

pub(super) fn save_spool(path: &Path, spool: &SpoolState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create syslog spool dir {}", parent.display()))?;
    }
    let temp = path.with_extension(format!("tmp-{}-{}", std::process::id(), random_u64()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| {
        let mut file = options
            .open(&temp)
            .with_context(|| format!("create syslog spool {}", temp.display()))?;
        file.write_all(&serde_json::to_vec(spool)?)
            .with_context(|| format!("write syslog spool {}", temp.display()))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path).with_context(|| format!("replace syslog spool {}", path.display()))
    })();
    if let Err(primary_error) = &result
        && let Err(cleanup_error) = fs::remove_file(&temp)
        && cleanup_error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::error!(
            temp = %temp.display(),
            error = %cleanup_error,
            primary_error = format!("{primary_error:#}"),
            "failed to clean up syslog spool temporary file"
        );
    }
    result
}

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
