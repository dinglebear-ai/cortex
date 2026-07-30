//! Centralised heartbeat flag computation.
//!
//! All threshold-based pressure signals are derived here so that MCP, REST,
//! and CLI adapters share identical logic. No adapter may own fleet policy.
//!
//! ## Pressure thresholds
//!
//! | Signal | Threshold |
//! |---|---|
//! | `cpu_pressure` | `usage_percent > 90 %` |
//! | `memory_pressure` | `used_percent > 90 %` |
//! | `swap_pressure` | `swap_used / swap_total > 90 %` (when swap is present) |
//! | `disk_capacity_pressure` | any disk `used_percent > 90 %` |
//! | `network_error_pressure` | any interface `rx_errors + tx_errors > 0` |
//! | `container_unhealthy` | any container runtime reports `unhealthy > 0` |
//! | `heartbeat_late` | no accepted heartbeat for `> 2.5 × agent interval` |
//! | `clock_skew` | Absolute difference between `sampled_at` and `received_at` exceeds 30 seconds |

use serde_json::Value;

use crate::db::{
    HeartbeatLatestEntry, HeartbeatMetricSnapshot, HeartbeatSampleState, HeartbeatStateFlags,
};

// ── Threshold constants ───────────────────────────────────────────────────

const CPU_PRESSURE_THRESHOLD: f64 = 90.0;
const MEM_PRESSURE_THRESHOLD: f64 = 90.0;
const SWAP_PRESSURE_RATIO: f64 = 0.9;
const DISK_CAPACITY_THRESHOLD: f64 = 90.0;
/// 2.5× the agent's declared interval in milliseconds before a heartbeat is
/// considered late (matches the contract: §7 "Default late heartbeat threshold").
const LATE_MULTIPLIER_MS: i64 = 2500;
/// Maximum |sampled_at − received_at| in seconds before clock_skew is flagged.
const CLOCK_SKEW_THRESHOLD_SECS: i64 = 30;

// ── Public API ────────────────────────────────────────────────────────────

/// Derive flags from a fully-loaded `HeartbeatSampleState` (single-host path).
///
/// All metric data is already embedded in the sample as JSON Values, so no
/// additional DB queries are needed.
pub fn from_sample(sample: &HeartbeatSampleState) -> HeartbeatStateFlags {
    let interval_secs = sample
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/agent/interval_secs"))
        .and_then(Value::as_i64)
        .unwrap_or(30)
        .max(1);

    let heartbeat_late = compute_late(&sample.received_at, interval_secs);
    let clock_skew = compute_clock_skew(&sample.sampled_at, &sample.received_at);

    let cpu_usage = sample
        .cpu
        .as_ref()
        .and_then(|c| c["usage_percent"].as_f64());
    let mem_used = sample
        .memory
        .as_ref()
        .and_then(|m| m["used_percent"].as_f64());
    let swap_total = sample
        .memory
        .as_ref()
        .and_then(|m| m["swap_total_bytes"].as_i64());
    let swap_used = sample
        .memory
        .as_ref()
        .and_then(|m| m["swap_used_bytes"].as_i64());
    let max_disk = sample
        .disks
        .iter()
        .filter_map(disk_pressure_used_percent)
        .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))));
    let net_errors: i64 = sample
        .network
        .iter()
        .map(|n| n["rx_errors"].as_i64().unwrap_or(0) + n["tx_errors"].as_i64().unwrap_or(0))
        .sum();
    let container_unhealthy = sample
        .containers
        .iter()
        .any(|c| c["unhealthy"].as_i64().unwrap_or(0) > 0);

    HeartbeatStateFlags {
        collector_partial: sample.partial,
        heartbeat_late,
        clock_skew,
        cpu_pressure: cpu_usage.is_some_and(|p| p > CPU_PRESSURE_THRESHOLD),
        memory_pressure: mem_used.is_some_and(|p| p > MEM_PRESSURE_THRESHOLD),
        swap_pressure: swap_ratio(swap_total, swap_used),
        disk_capacity_pressure: max_disk.is_some_and(|p| p > DISK_CAPACITY_THRESHOLD),
        network_error_pressure: net_errors > 0,
        container_unhealthy,
    }
}

/// Derive flags from a `HeartbeatLatestEntry` + `HeartbeatMetricSnapshot`
/// (fleet-state path). The entry supplies timing/metadata; the snapshot
/// supplies aggregated metric values fetched from the child tables.
pub fn from_latest_and_metrics(
    entry: &HeartbeatLatestEntry,
    metrics: &HeartbeatMetricSnapshot,
) -> HeartbeatStateFlags {
    let interval_secs = entry
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|v| v.pointer("/agent/interval_secs").and_then(Value::as_i64))
        .unwrap_or(30)
        .max(1);

    let heartbeat_late = compute_late(&entry.received_at, interval_secs);
    let clock_skew = compute_clock_skew(&entry.sampled_at, &entry.received_at);

    HeartbeatStateFlags {
        collector_partial: entry.partial,
        heartbeat_late,
        clock_skew,
        cpu_pressure: metrics
            .cpu_usage_percent
            .is_some_and(|p| p > CPU_PRESSURE_THRESHOLD),
        memory_pressure: metrics
            .mem_used_percent
            .is_some_and(|p| p > MEM_PRESSURE_THRESHOLD),
        swap_pressure: swap_ratio(metrics.swap_total_bytes, metrics.swap_used_bytes),
        disk_capacity_pressure: metrics
            .max_disk_used_percent
            .is_some_and(|p| p > DISK_CAPACITY_THRESHOLD),
        network_error_pressure: metrics.total_network_errors.is_some_and(|e| e > 0),
        container_unhealthy: metrics.container_unhealthy_count.is_some_and(|c| c > 0),
    }
}

/// Active pressure signal names (excludes availability signals like
/// `heartbeat_late`, `clock_skew`, `collector_partial`).
pub fn pressure_names(flags: &HeartbeatStateFlags) -> Vec<String> {
    let mut names = Vec::new();
    if flags.cpu_pressure {
        names.push("cpu_pressure".to_owned());
    }
    if flags.memory_pressure {
        names.push("memory_pressure".to_owned());
    }
    if flags.swap_pressure {
        names.push("swap_pressure".to_owned());
    }
    if flags.disk_capacity_pressure {
        names.push("disk_capacity_pressure".to_owned());
    }
    if flags.network_error_pressure {
        names.push("network_error_pressure".to_owned());
    }
    if flags.container_unhealthy {
        names.push("container_unhealthy".to_owned());
    }
    names
}

/// Canonical status label for a host based on its derived flags.
///
/// Priority: `late` > `partial` > `pressure` > `ok`.
pub fn host_status_label(flags: &HeartbeatStateFlags) -> &'static str {
    let has_pressure = flags.cpu_pressure
        || flags.memory_pressure
        || flags.swap_pressure
        || flags.disk_capacity_pressure
        || flags.network_error_pressure
        || flags.container_unhealthy;
    if flags.heartbeat_late {
        "late"
    } else if flags.collector_partial {
        "partial"
    } else if has_pressure {
        "pressure"
    } else {
        "ok"
    }
}

// ── Private helpers ───────────────────────────────────────────────────────

fn compute_late(received_at: &str, interval_secs: i64) -> bool {
    let interval_secs = interval_secs.max(1);
    chrono::DateTime::parse_from_rfc3339(received_at).is_ok_and(|dt| {
        let elapsed = chrono::Utc::now().signed_duration_since(dt.with_timezone(&chrono::Utc));
        elapsed.num_milliseconds() > interval_secs * LATE_MULTIPLIER_MS
    })
}

fn compute_clock_skew(sampled_at: &str, received_at: &str) -> bool {
    let sampled = chrono::DateTime::parse_from_rfc3339(sampled_at).ok();
    let received = chrono::DateTime::parse_from_rfc3339(received_at).ok();
    match (sampled, received) {
        (Some(s), Some(r)) => {
            let skew = s.with_timezone(&chrono::Utc) - r.with_timezone(&chrono::Utc);
            skew.num_seconds().abs() > CLOCK_SKEW_THRESHOLD_SECS
        }
        _ => false,
    }
}

fn swap_ratio(swap_total: Option<i64>, swap_used: Option<i64>) -> bool {
    match (swap_total, swap_used) {
        (Some(total), Some(used)) if total > 0 => {
            (used as f64 / total as f64) > SWAP_PRESSURE_RATIO
        }
        _ => false,
    }
}

fn disk_pressure_used_percent(disk: &Value) -> Option<f64> {
    if !is_pressure_relevant_disk(disk) {
        return None;
    }
    disk["used_percent"].as_f64()
}

pub(crate) fn is_pressure_relevant_disk(disk: &Value) -> bool {
    let fs = disk["filesystem"]
        .as_str()
        .or_else(|| disk["fs_type"].as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mount = disk["mountpoint"]
        .as_str()
        .or_else(|| disk["name"].as_str())
        .unwrap_or("");

    if matches!(
        fs.as_str(),
        "autofs"
            | "binfmt_misc"
            | "bpf"
            | "cgroup"
            | "cgroup2"
            | "configfs"
            | "debugfs"
            | "devpts"
            | "devtmpfs"
            | "efivarfs"
            | "fuse.snapfuse"
            | "fusectl"
            | "hugetlbfs"
            | "iso9660"
            | "mqueue"
            | "nsfs"
            | "overlay"
            | "proc"
            | "pstore"
            | "ramfs"
            | "rootfs"
            | "securityfs"
            | "squashfs"
            | "sysfs"
            | "tmpfs"
            | "tracefs"
    ) {
        return false;
    }

    !matches!(mount, "" | "/init")
        && !mount.starts_with("/snap/")
        && !mount.starts_with("/mnt/wsl/docker-desktop/")
        && !mount.starts_with("/mnt/wslg/")
        && !mount.starts_with("/usr/lib/modules/")
        && !mount.starts_with("/usr/lib/wsl/")
        && !mount.starts_with("/run/")
        && !mount.starts_with("/var/run/")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn disk_pressure_ignores_read_only_image_mounts() {
        let disk = json!({
            "filesystem": "iso9660",
            "mountpoint": "/mnt/wsl/docker-desktop/cli-tools",
            "used_percent": 100.0
        });

        assert_eq!(disk_pressure_used_percent(&disk), None);
    }

    #[test]
    fn disk_pressure_keeps_real_writable_mounts() {
        let disk = json!({
            "filesystem": "ext4",
            "mountpoint": "/",
            "used_percent": 95.0
        });

        assert_eq!(disk_pressure_used_percent(&disk), Some(95.0));
    }

    #[test]
    fn disk_pressure_keeps_unraid_user_share_mounts() {
        let disk = json!({
            "filesystem": "fuse.shfs",
            "mountpoint": "/mnt/user/appdata/cortex",
            "used_percent": 95.0
        });

        assert_eq!(disk_pressure_used_percent(&disk), Some(95.0));
    }
}
