use super::*;

#[test]
fn snapshot_reports_queue_utilization() {
    let obs = RuntimeObservability::default();
    obs.set_queue_capacity(100);
    obs.set_queue_depth(25);

    let snapshot = obs.snapshot();

    assert_eq!(snapshot.ingest_queue_depth, 25);
    assert_eq!(snapshot.ingest_queue_capacity, 100);
    assert_eq!(snapshot.ingest_queue_utilization_pct, "25.00");
}

#[test]
fn tcp_connection_active_count_saturates() {
    let obs = RuntimeObservability::default();
    obs.record_tcp_connection_closed();
    assert_eq!(obs.snapshot().syslog_tcp_connections_active, 0);

    obs.record_tcp_connection_accepted();
    obs.record_tcp_connection_closed();
    assert_eq!(obs.snapshot().syslog_tcp_connections_active, 0);
}

#[test]
fn snapshot_reports_docker_ingest_counters() {
    let obs = RuntimeObservability::default();
    obs.record_docker_ingest_event();
    obs.record_docker_ingest_log_entry();
    obs.record_docker_ingest_parse_error();
    obs.record_docker_ingest_stream_reconnect();
    obs.record_docker_ingest_stream_failure();
    obs.record_docker_ingest_task_spawned();
    obs.record_docker_ingest_host_stream_started();
    obs.record_docker_ingest_container_stream_started();
    obs.record_remote_docker_event_stream_failure("nashost", "exit status 255");

    let snapshot = obs.snapshot();

    assert_eq!(snapshot.docker_ingest_events_received, 1);
    assert_eq!(snapshot.docker_ingest_log_entries_received, 1);
    assert_eq!(snapshot.docker_ingest_parse_errors, 1);
    assert_eq!(snapshot.docker_ingest_stream_reconnects, 1);
    assert_eq!(snapshot.docker_ingest_stream_failures, 1);
    assert_eq!(snapshot.docker_ingest_tasks_spawned, 1);
    assert_eq!(snapshot.docker_ingest_host_streams_active, 1);
    assert_eq!(snapshot.docker_ingest_container_streams_active, 1);
    assert_eq!(snapshot.remote_docker_event_stream_failures, 1);
    assert!(snapshot.last_ingest_at.is_some());
    assert!(snapshot.last_error_at.is_some());
    assert!(snapshot.last_docker_ingest_event_at.is_some());
    assert!(snapshot.last_docker_ingest_log_at.is_some());
    assert!(snapshot.last_docker_ingest_error_at.is_some());
    assert!(snapshot.last_remote_docker_event_stream_error_at.is_some());
    assert_eq!(
        snapshot.last_remote_docker_event_stream_error.as_deref(),
        Some("nashost: exit status 255")
    );

    obs.record_docker_ingest_host_stream_ended();
    obs.record_docker_ingest_host_stream_ended();
    obs.record_docker_ingest_container_stream_ended();
    obs.record_docker_ingest_container_stream_ended();

    let snapshot = obs.snapshot();
    assert_eq!(snapshot.docker_ingest_host_streams_active, 0);
    assert_eq!(snapshot.docker_ingest_container_streams_active, 0);
}

#[test]
fn snapshot_reports_queue_pressure_counters() {
    let obs = RuntimeObservability::default();
    obs.record_write_channel_full_transition();
    obs.record_udp_packet_dropped_queue_full(10);
    obs.record_tcp_line_dropped_queue_full(20);

    let snapshot = obs.snapshot();

    assert_eq!(snapshot.syslog_write_channel_full_transitions, 1);
    assert_eq!(snapshot.syslog_udp_packets_dropped_queue_full, 1);
    assert_eq!(snapshot.syslog_tcp_lines_dropped_queue_full, 1);
    assert_eq!(snapshot.ingest_queue_depth, 20);
    assert!(snapshot.last_error_at.is_some());
}
