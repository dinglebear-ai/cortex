use super::*;

/// Discover one provider root without granting the worker any persistence
/// capability. A root that blocks in filesystem discovery can be abandoned at
/// the deadline; a later provider root still receives its own bounded turn.
pub(super) fn discover_root_bounded(
    path: PathBuf,
    start_after: Option<&Path>,
    budget: DiscoveryBudget,
) -> Result<DiscoveryResult> {
    let Some(worker_slot) = WorkerSlot::acquire(&DISCOVERY_WORKERS_IN_FLIGHT) else {
        return Ok(DiscoveryResult {
            capped: true,
            ..Default::default()
        });
    };
    let deadline = budget.deadline;
    let start_after = start_after.map(Path::to_path_buf);
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("cortex-transcript-discovery".to_string())
        .spawn(move || {
            let _worker_slot = worker_slot;
            #[cfg(test)]
            hold_snapshot_for_test(&path);
            let mut result = DiscoveryResult::default();
            let started = Instant::now();
            let mut entries = 0usize;
            let mut path_bytes = 0usize;
            collect_supported_files_bounded(
                &path,
                &mut result,
                &mut entries,
                &mut path_bytes,
                start_after.as_deref(),
                &budget,
                started,
            );
            let _ = sender.send(result);
        });
    #[cfg(test)]
    let worker = worker.map(|handle| {
        if SNAPSHOT_TEST_HOLDS.lock().unwrap().is_some() {
            SNAPSHOT_TEST_WORKERS.lock().unwrap().push(handle);
        }
    });
    if worker.is_err() {
        return Err(anyhow::anyhow!(
            "could not start bounded transcript discovery"
        ));
    }
    match receiver.recv_timeout(deadline) {
        Ok(result) => Ok(result),
        Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
            Ok(DiscoveryResult {
                capped: true,
                ..Default::default()
            })
        }
    }
}

/// Private scheduling inputs shared by the watcher and service boundary.
#[derive(Default)]
pub(crate) struct DiscoveryScanState {
    pub(crate) scan_budget: Option<ScanBudget>,
    pub(crate) start_after: Option<PathBuf>,
    pub(crate) discovery_start_after: BTreeMap<PathBuf, PathBuf>,
    pub(crate) continuation: Option<DiscoveryContinuation>,
}

/// Watcher-owned workers retain their directory iterators across rescan turns.
/// Only the worker touches filesystem state; this map holds receivers, not DB handles.
#[derive(Clone, Default)]
pub(crate) struct DiscoveryContinuation(std::sync::Arc<ContinuationState>);

#[derive(Default)]
struct ContinuationState {
    pending: std::sync::Mutex<BTreeMap<PathBuf, Pending>>,
    completed: std::sync::Mutex<HashSet<PathBuf>>,
    ready: std::sync::Mutex<BTreeMap<PathBuf, ReadyPage>>,
    cancelled: std::sync::atomic::AtomicBool,
}

struct ReadyPage {
    page: DiscoveryResult,
    identity: Option<(u64, u64, Option<std::time::SystemTime>)>,
}

struct Pending {
    receiver: mpsc::Receiver<Finished>,
    turn: std::sync::Arc<DiscoveryTurn>,
}

/// The worker pauses at entry boundaries, retaining its iterator and stack.
/// The mutex protects only scheduling; no filesystem syscall holds it.
pub(super) struct DiscoveryTurn {
    state: std::sync::Mutex<(Instant, bool)>,
    ready: std::sync::Condvar,
    duration: Duration,
}

impl DiscoveryTurn {
    fn new(duration: Duration) -> Self {
        Self {
            state: std::sync::Mutex::new((Instant::now(), false)),
            ready: std::sync::Condvar::new(),
            duration,
        }
    }

    fn resume(&self) {
        self.state.lock().unwrap().0 = Instant::now();
        self.ready.notify_all();
    }

    fn cancel(&self) {
        self.state.lock().unwrap().1 = true;
        self.ready.notify_all();
    }

    pub(super) fn checkpoint(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        while !state.1 && state.0.elapsed() >= self.duration {
            state = self.ready.wait(state).unwrap();
        }
        state.1
    }
}

struct Finished {
    result: DiscoveryResult,
    identity: Option<(u64, u64, Option<std::time::SystemTime>)>,
}

impl std::fmt::Debug for DiscoveryContinuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryContinuation")
            .finish_non_exhaustive()
    }
}

impl Drop for ContinuationState {
    fn drop(&mut self) {
        // A blocked syscall still owns its admission slot until it returns.
        for pending in self.pending.get_mut().unwrap().values() {
            pending.turn.cancel();
        }
    }
}

impl DiscoveryContinuation {
    pub(crate) fn clear(&mut self) {
        self.cancel_all();
        *self = Self::default();
    }

    pub(crate) fn is_pending(&self) -> bool {
        !self.0.pending.lock().unwrap().is_empty() || !self.0.ready.lock().unwrap().is_empty()
    }

    pub(super) fn cancel_all(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let pending = std::mem::take(&mut *self.0.pending.lock().unwrap());
        for active in pending.into_values() {
            active.turn.cancel();
        }
        self.0.ready.lock().unwrap().clear();
        self.0.completed.lock().unwrap().clear();
    }

    pub(super) fn retain_roots(&self, roots: &[PathBuf]) {
        let removed: Vec<_> = self
            .0
            .pending
            .lock()
            .unwrap()
            .keys()
            .filter(|path| !roots.contains(path))
            .cloned()
            .collect();
        for root in removed {
            self.forget_root(&root);
        }
        self.0
            .ready
            .lock()
            .unwrap()
            .retain(|root, _| roots.contains(root));
        self.0
            .completed
            .lock()
            .unwrap()
            .retain(|root| roots.contains(root));
    }

    fn offer_page(page: &mut DiscoveryResult, start_after: Option<&Path>) -> DiscoveryResult {
        DiscoveryResult {
            files: page.files.clone(),
            diagnostics: std::mem::take(&mut page.diagnostics),
            // Discovery cannot advance until the page's sources acknowledge completion.
            capped: true,
            next_cursor: start_after.map(Path::to_path_buf),
        }
    }

    pub(super) fn forget_root(&self, root: &Path) {
        if let Some(pending) = self.0.pending.lock().unwrap().remove(root) {
            pending.turn.cancel();
        }
        self.0.ready.lock().unwrap().remove(root);
        self.0.completed.lock().unwrap().remove(root);
    }

    pub(super) fn source_finished(&self, file: &Path) {
        for page in self.0.ready.lock().unwrap().values_mut() {
            page.page.files.retain(|path| path != file);
        }
    }

    pub(super) fn acknowledge_source(&self, file: &Path, result: &IndexResult) {
        if result.source_budget_cap_hits == 0
            && result.source_deadline_exceeded == 0
            && result.snapshot_capacity_deferred == 0
            && result.deferred_sources == 0
            && !result.scan_budget_cap_hit
            && result.storage_blocked_chunks == 0
        {
            self.source_finished(file);
        }
    }

    pub(super) fn finish_pass(&self, result: &mut IndexResult) {
        let mut ready = self.0.ready.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) {
            return;
        }
        ready.retain(|root, ready| {
            let page = &ready.page;
            if !page.files.is_empty() {
                return true;
            }
            if page.capped {
                if let Some(cursor) = &page.next_cursor {
                    result
                        .next_discovery_cursors
                        .insert(root.clone(), cursor.clone());
                }
            } else {
                self.0.completed.lock().unwrap().insert(root.clone());
                if result.next_discovery_cursors.remove(root).is_some() {
                    result.discovery_cap_hits = result.discovery_cap_hits.saturating_sub(1);
                    result.discovery_deferred_roots =
                        result.discovery_deferred_roots.saturating_sub(1);
                }
            }
            false
        });
    }

    pub(super) fn discover(
        &self,
        path: PathBuf,
        start_after: Option<&Path>,
        mut budget: DiscoveryBudget,
    ) -> Result<DiscoveryResult> {
        if self.0.completed.lock().unwrap().contains(&path) {
            return Ok(DiscoveryResult::default());
        }
        let deferred = || DiscoveryResult {
            capped: true,
            next_cursor: start_after.map(Path::to_path_buf),
            ..Default::default()
        };
        if self.0.cancelled.load(Ordering::Acquire) {
            return Ok(deferred());
        }
        // Validate identity without holding the page map across a filesystem call.
        let identity = discovery::directory_identity(&path);
        let mut ready = self.0.ready.lock().unwrap();
        if let Some(page) = ready.get_mut(&path) {
            if page.identity == identity {
                return Ok(Self::offer_page(&mut page.page, start_after));
            }
            ready.remove(&path);
            return Ok(deferred());
        }
        drop(ready);
        let mut pending = self.0.pending.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) {
            return Ok(deferred());
        }
        if let Some(active) = pending.get(&path) {
            active.turn.resume();
            let received = active.receiver.try_recv();
            match received {
                Ok(finished) => {
                    pending.remove(&path);
                    drop(pending);
                    // Replacement or mutation invalidates the unfinished page.
                    // Its receiver cannot publish into a newer generation.
                    if finished.identity != discovery::directory_identity(&path) {
                        return Ok(deferred());
                    }
                    let mut page = finished.result;
                    let offered = Self::offer_page(&mut page, start_after);
                    let mut ready = self.0.ready.lock().unwrap();
                    if self.0.cancelled.load(Ordering::Acquire) {
                        return Ok(deferred());
                    }
                    ready.insert(
                        path,
                        ReadyPage {
                            page,
                            identity: finished.identity,
                        },
                    );
                    return Ok(offered);
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(deferred()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    pending.remove(&path);
                }
            }
        }
        let Some(slot) = WorkerSlot::acquire(&DISCOVERY_WORKERS_IN_FLIGHT) else {
            return Ok(deferred());
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_path = path.clone();
        let cursor = start_after.map(Path::to_path_buf);
        let turn = std::sync::Arc::new(DiscoveryTurn::new(budget.deadline));
        budget.cancel = Some(turn.clone());
        // Inserting before spawning prevents a second turn from starting the same root.
        pending.insert(path.clone(), Pending { receiver, turn });
        drop(pending);
        let spawn = std::thread::Builder::new()
            .name("cortex-transcript-discovery".into())
            .spawn(move || {
                let _slot = slot;
                #[cfg(test)]
                hold_snapshot_for_test(&worker_path);
                let identity = discovery::directory_identity(&worker_path);
                let mut result = DiscoveryResult::default();
                collect_supported_files_bounded(
                    &worker_path,
                    &mut result,
                    &mut 0,
                    &mut 0,
                    cursor.as_deref(),
                    &budget,
                    Instant::now(),
                );
                let _ = sender.send(Finished { result, identity });
            });
        #[cfg(test)]
        let spawn = spawn.map(|handle| {
            if SNAPSHOT_TEST_HOLDS.lock().unwrap().is_some() {
                SNAPSHOT_TEST_WORKERS.lock().unwrap().push(handle);
            }
        });
        if spawn.is_err() {
            self.0.pending.lock().unwrap().remove(&path);
            return Err(anyhow::anyhow!(
                "could not start bounded transcript discovery"
            ));
        }
        Ok(deferred())
    }
}

#[cfg(test)]
#[path = "discovery_continuation_tests.rs"]
mod tests;
