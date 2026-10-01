use super::*;

#[test]
fn counter_rates_do_not_fabricate_reset_or_zero_interval_values() {
    assert_eq!(counter_rate(120, 100, Duration::from_secs(2)), Some(10.0));
    assert_eq!(counter_rate(100, 120, Duration::from_secs(2)), None);
    assert_eq!(counter_rate(120, 100, Duration::ZERO), None);
    assert_eq!(bytes(u64::MAX), i64::MAX);
}

#[tokio::test]
async fn native_worker_rejects_overlap_even_after_caller_timeout() {
    let probe = NativeResourceProbe::new();
    // Stall the blocking worker after it acquires its permit, then cancel its
    // async caller. The permit must remain with that worker, not the caller.
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let state = probe.state.clone();
    let holder = tokio::task::spawn_blocking(move || {
        let _locked = state.lock().unwrap();
        ready_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    ready_rx.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), probe.collect())
            .await
            .is_err()
    );
    let error = match probe.collect().await {
        Err(error) => error,
        Ok(_) => panic!("overlap accepted"),
    };
    assert!(error.to_string().contains("still running"));
    release_tx.send(()).unwrap();
    holder.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while probe.gate.available_permits() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_platform_collects_real_resources_and_network_baselines() {
    let collector = HeartbeatCollector::for_platform(std::env::consts::OS);
    let collect = || {
        collector.collect(
            "native-test-host".to_string(),
            1,
            Duration::from_secs(30),
            0,
            Duration::from_secs(5),
            Duration::from_secs(6),
        )
    };
    let first = collect().await;
    assert!(!first.sample.partial, "{:?}", first.sample.probe_errors);
    let cpu = first.cpu.unwrap();
    assert!(cpu.core_count > 0);
    assert!((0.0..=100.0).contains(&cpu.usage_pct.unwrap()));
    let memory = first.memory.unwrap();
    assert!(memory.mem_total_bytes > 0);
    assert!(memory.mem_available_bytes <= memory.mem_total_bytes);
    assert!(first.processes.unwrap().total > 0);
    assert!(!first.disks.is_empty());
    assert!(!first.networks.is_empty());
    assert!(first.networks.iter().all(|n| n.rx_bytes_per_sec.is_none()));
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    let second = collect().await;
    assert!(!second.sample.partial, "{:?}", second.sample.probe_errors);
    assert!(second.networks.iter().any(|n| n.rx_bytes_per_sec.is_some()));
    for disk in second.disks {
        assert!(disk.bytes_free <= disk.bytes_total);
        assert_eq!(
            disk.bytes_used.unwrap() + disk.bytes_free.unwrap(),
            disk.bytes_total.unwrap()
        );
    }
}

#[test]
fn interface_limit_preserves_traffic_bearing_tunnels_over_idle_adapters() {
    let mut interfaces: Vec<_> = (0..20).map(|i| (format!("en{i:02}"), 0, 0)).collect();
    interfaces.push(("utun9".to_string(), 1200, 800));
    interfaces.push(("en0".to_string(), u64::MAX, 1));
    interfaces.sort_by_cached_key(|(name, rx, tx)| network_priority(name, *rx, *tx));
    let selected: Vec<_> = interfaces.iter().take(16).map(|i| i.0.as_str()).collect();
    assert_eq!(&selected[..2], &["en0", "utun9"]);
    assert_eq!(&selected[2..4], &["en00", "en01"]);
}
