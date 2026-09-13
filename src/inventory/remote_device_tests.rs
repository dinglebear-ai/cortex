use super::*;

#[test]
fn normalizes_remote_device_facts() {
    let mut out = CollectorOutput::new("remote_device");
    normalize_host(
        "devhost",
        "hostname=devhost\nos=Ubuntu\ncpu=Intel\nmemory=16Gi\nip=192.0.2.6\nstorage=ext4\t100\t50\t/\nlistener=tcp LISTEN 0 128 0.0.0.0:3100 0.0.0.0:*\n",
        &mut out,
    );

    assert_eq!(out.nodes[0].hostname, "devhost");
    assert_eq!(out.nodes[0].ips, vec!["192.0.2.6"]);
    assert_eq!(out.nodes[0].listeners[0].port, Some(3100));
    assert_eq!(out.storage[0].mount, "/");
}

#[tokio::test]
async fn cancelling_collectors_drops_active_probes_and_queued_hosts() {
    use crate::inventory::ssh::SshOptions;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    for docker in [false, true] {
        let active = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let context = SshContext::with_runner_for_test(
            SshOptions::default().with_max_concurrent(1).unwrap(),
            {
                let active = active.clone();
                let starts = starts.clone();
                let started = started.clone();
                move |_, _, _| {
                    let active = active.clone();
                    let starts = starts.clone();
                    let started = started.clone();
                    Box::pin(async move {
                        active.fetch_add(1, Ordering::SeqCst);
                        let _active = Active(active);
                        starts.fetch_add(1, Ordering::SeqCst);
                        started.notify_one();
                        std::future::pending().await
                    })
                }
            },
        );
        let hosts = vec!["host-a".to_string(), "host-b".to_string()];
        let mut future: std::pin::Pin<Box<dyn std::future::Future<Output = CollectorOutput>>> =
            if docker {
                Box::pin(crate::inventory::remote_docker::collect(
                    None,
                    &hosts,
                    &context,
                    Duration::from_secs(30),
                ))
            } else {
                Box::pin(collect(None, &hosts, &context, Duration::from_secs(30)))
            };
        tokio::select! {
            _ = &mut future => panic!("held collector completed"),
            _ = started.notified() => {},
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("probe never started"),
        }
        drop(future);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }
}
