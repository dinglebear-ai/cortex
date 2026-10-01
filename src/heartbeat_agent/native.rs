//! Native resource collection for desktop hosts. Blocking OS calls run off the
//! async executor; a permit remains with the worker if its caller times out.
use super::*;
use sysinfo::{CpuRefreshKind, Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::Semaphore;

pub struct ResourceSnapshot {
    pub cpu: HeartbeatCpu,
    pub memory: HeartbeatMemory,
    pub disks: Vec<HeartbeatDisk>,
    pub networks: Vec<HeartbeatNetwork>,
    pub processes: HeartbeatProcesses,
    pub errors: Vec<String>,
}

pub(super) struct NativeResourceProbe {
    state: Arc<Mutex<NativeState>>,
    gate: Arc<Semaphore>,
}

impl NativeResourceProbe {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(NativeState::default())),
            gate: Arc::new(Semaphore::new(1)),
        }
    }
}

impl HeartbeatProbe for NativeResourceProbe {
    fn name(&self) -> &'static str {
        "native_resources"
    }

    fn collect(&self) -> Pin<Box<dyn Future<Output = Result<ProbeOutput>> + Send + '_>> {
        Box::pin(async move {
            let permit = self
                .gate
                .clone()
                .try_acquire_owned()
                .context("previous native resource collection is still running")?;
            let state = self.state.clone();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                state
                    .lock()
                    .map_err(|_| anyhow!("native resource state poisoned"))?
                    .collect()
                    .map(ProbeOutput::Resources)
            })
            .await
            .context("native resource worker failed")?
        })
    }
}

#[derive(Default)]
struct NativeState {
    system: System,
    disks: Disks,
    networks: Networks,
    previous_disks: BTreeMap<String, (Instant, [u64; 2])>,
    previous_networks: BTreeMap<String, (Instant, [u64; 4])>,
    cpu_ready: bool,
}

fn bytes(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn network_priority(
    name: &str,
    received: u64,
    transmitted: u64,
) -> (std::cmp::Reverse<u64>, String) {
    (
        std::cmp::Reverse(received.saturating_add(transmitted)),
        name.to_owned(),
    )
}

fn counter_rate(current: u64, previous: u64, elapsed: Duration) -> Option<f64> {
    if elapsed.is_zero() {
        return None;
    }
    current
        .checked_sub(previous)
        .map(|delta| delta as f64 / elapsed.as_secs_f64())
}

impl NativeState {
    fn collect(&mut self) -> Result<ResourceSnapshot> {
        if !self.cpu_ready {
            self.system
                .refresh_cpu_list(CpuRefreshKind::nothing().with_cpu_usage());
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            self.cpu_ready = true;
        }
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        if self.system.cpus().is_empty() || self.system.total_memory() == 0 {
            bail!("native CPU or memory information unavailable");
        }
        let load = System::load_average();
        let usage = self.system.global_cpu_usage() as f64;
        if !usage.is_finite() || !(0.0..=100.0).contains(&usage) {
            bail!("invalid native CPU utilization");
        }
        let cpu = HeartbeatCpu {
            // Windows has no Unix load averages. The v1 protocol requires these
            // fields; use its legacy zeros and report measured utilization.
            load1: load.one,
            load5: load.five,
            load15: load.fifteen,
            usage_pct: Some(usage),
            user_pct: None,
            system_pct: None,
            iowait_pct: None,
            steal_pct: None,
            core_count: self.system.cpus().len() as i64,
        };
        let memory = HeartbeatMemory {
            mem_total_bytes: bytes(self.system.total_memory()),
            mem_available_bytes: bytes(self.system.available_memory()),
            mem_used_bytes: Some(bytes(self.system.used_memory())),
            swap_total_bytes: bytes(self.system.total_swap()),
            swap_used_bytes: bytes(self.system.used_swap()),
        };
        // Refresh discovery every cycle so hot-plugged volumes and interfaces
        // appear and removed devices do not keep producing stale metrics.
        self.disks.refresh(true);
        let mut errors = Vec::new();
        let mut disks = Vec::new();
        let mut previous_disks = BTreeMap::new();
        let disk_now = Instant::now();
        for disk in self.disks.list_mut().iter_mut().take(16) {
            if !disk.refresh() || disk.total_space() == 0 {
                errors.push("native disk capacity unavailable".to_string());
                continue;
            }
            let total = disk.total_space();
            let free = disk.available_space().min(total);
            let name = disk.mount_point().to_string_lossy().into_owned();
            let usage = disk.usage();
            let counters = [usage.total_read_bytes, usage.total_written_bytes];
            let identity = format!("{}:{}", name, disk.name().to_string_lossy());
            let rates = self
                .previous_disks
                .get(&identity)
                .filter(|(_, old)| counters != [0, 0] || *old != [0, 0])
                .map(|(at, old)| {
                    std::array::from_fn::<_, 2, _>(|i| {
                        counter_rate(counters[i], old[i], disk_now.duration_since(*at))
                    })
                })
                .unwrap_or([None; 2]);
            previous_disks.insert(identity, (disk_now, counters));
            disks.push(HeartbeatDisk {
                kind: "mount".to_string(),
                name,
                fs_type: Some(disk.file_system().to_string_lossy().into_owned()),
                bytes_total: Some(bytes(total)),
                bytes_free: Some(bytes(free)),
                bytes_used: Some(bytes(total - free)),
                read_bytes_per_sec: rates[0],
                write_bytes_per_sec: rates[1],
            });
        }
        self.previous_disks = previous_disks;
        if disks.is_empty() {
            errors.push("no native disk capacity samples".to_string());
        }
        self.networks.refresh(true);
        let now = Instant::now();
        let mut previous = BTreeMap::new();
        let mut interfaces: Vec<_> = self.networks.iter().collect();
        interfaces.sort_by_cached_key(|(name, data)| {
            network_priority(name, data.total_received(), data.total_transmitted())
        });
        let networks = interfaces
            .into_iter()
            .take(16)
            .map(|(name, data)| {
                let counters = [
                    data.total_received(),
                    data.total_transmitted(),
                    data.total_errors_on_received(),
                    data.total_errors_on_transmitted(),
                ];
                let rates = self
                    .previous_networks
                    .get(name)
                    .map(|(at, old)| {
                        std::array::from_fn::<_, 4, _>(|i| {
                            counter_rate(counters[i], old[i], now.duration_since(*at))
                        })
                    })
                    .unwrap_or([None; 4]);
                previous.insert(name.clone(), (now, counters));
                HeartbeatNetwork {
                    interface: name.clone(),
                    rx_bytes_per_sec: rates[0],
                    tx_bytes_per_sec: rates[1],
                    rx_errors_per_sec: rates[2],
                    tx_errors_per_sec: rates[3],
                }
            })
            .collect();
        self.previous_networks = previous;
        let processes = HeartbeatProcesses {
            total: self.system.processes().len() as i64,
            // Windows process states are not comparable to Unix scheduler states.
            running: None,
            sleeping: None,
            zombies: 0,
            top: Vec::new(),
        };
        Ok(ResourceSnapshot {
            cpu,
            memory,
            disks,
            networks,
            processes,
            errors,
        })
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
