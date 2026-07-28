use super::*;
use std::path::PathBuf;

#[test]
fn ensure_command_success_accepts_zero_exit_without_timeout() {
    let output = cortex::compose::CommandOutput {
        exit_status: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        timed_out: false,
        timeout_cleanup: None,
    };

    ensure_command_success(&output).unwrap();
}

#[test]
fn ensure_command_success_reports_failed_status_and_stderr() {
    let output = cortex::compose::CommandOutput {
        exit_status: Some(1),
        stdout: String::new(),
        stderr: "bad compose".to_string(),
        stdout_truncated: false,
        stderr_truncated: false,
        timed_out: false,
        timeout_cleanup: None,
    };

    let err = ensure_command_success(&output).unwrap_err().to_string();
    assert!(err.contains("status=Some(1)"));
    assert!(err.contains("bad compose"));
}

#[test]
fn human_db_maintenance_outputs_accept_representative_payloads() {
    let coordination = vec![
        SetupPhase {
            name: "network",
            status: SetupStatus::Ok,
            detail: "ready".to_string(),
        },
        SetupPhase {
            name: "listener",
            status: SetupStatus::Warn,
            detail: "not running".to_string(),
        },
    ];
    print_db_status_response(
        &cortex::app::DbMaintenanceStatus {
            db_path: PathBuf::from("/tmp/cortex.db"),
            page_count: 10,
            freelist_count: 1,
            page_size: 4096,
            logical_size_bytes: 40960,
            physical_size_bytes: 81920,
            wal_size_bytes: Some(100),
            shm_size_bytes: Some(200),
            sqlite_page_cache_mb: 128,
            sqlite_page_cache_kib_per_connection: -16_384,
            sqlite_mmap_mb: 256,
            sqlite_mmap_bytes: 256 * 1024 * 1024,
            heavy_read_concurrency: 1,
            wal_checkpoint_mb: 256,
            wal_checkpoint_threshold_bytes: 256 * 1024 * 1024,
            cgroup_memory_status: "ok".to_string(),
            cgroup_memory_max_bytes: Some(2 * 1024 * 1024 * 1024),
            cgroup_memory_current_bytes: Some(512 * 1024 * 1024),
            cgroup_memory_peak_bytes: Some(1024 * 1024 * 1024),
            auto_vacuum: 0,
            journal_mode: "wal".to_string(),
            integrity_ok: Some(false),
            integrity_messages: vec!["row missing".to_string()],
        },
        Some(&coordination),
        false,
    )
    .unwrap();

    print_db_integrity_response(
        &cortex::app::DbIntegrityResult {
            ok: false,
            messages: vec!["bad".to_string()],
        },
        false,
    )
    .unwrap();
    print_db_integrity_job_started(
        &cortex::app::DbIntegrityJobStarted {
            job_id: 7,
            status: "running".to_string(),
        },
        false,
    )
    .unwrap();
    print_db_integrity_job_status(
        &cortex::app::MaintenanceJobStatus {
            job_id: 7,
            kind: "integrity".to_string(),
            status: "failed".to_string(),
            started_at: "2026-06-13T00:00:00Z".to_string(),
            finished_at: Some("2026-06-13T00:01:00Z".to_string()),
            integrity: Some(cortex::app::DbIntegrityResult {
                ok: true,
                messages: vec!["ok".to_string()],
            }),
            error: Some("boom".to_string()),
        },
        false,
    )
    .unwrap();
    print_db_checkpoint_response(
        &cortex::app::DbCheckpointResult {
            mode: "passive".to_string(),
            busy: 0,
            log_frames: 3,
            checkpointed_frames: 3,
            complete: true,
        },
        false,
    )
    .unwrap();
    print_db_vacuum_response(
        &cortex::app::DbVacuumResult {
            full: true,
            incremental_pages: 0,
            before_physical_size_bytes: 1000,
            after_physical_size_bytes: 900,
        },
        false,
    )
    .unwrap();
    print_db_backup_response(
        &cortex::app::DbBackupResult {
            db_path: PathBuf::from("/tmp/cortex.db"),
            backup_path: PathBuf::from("/tmp/cortex.backup.db"),
            size_bytes: 1234,
        },
        false,
    )
    .unwrap();
}

#[test]
fn human_compose_outputs_accept_status_doctor_dry_run_and_executed_payloads() {
    let status = cortex::compose::ComposeStatus {
        container_name: "cortex".to_string(),
        container_id: Some("abc".to_string()),
        status: Some("running".to_string()),
        health: Some("healthy".to_string()),
        image: Some("ghcr.io/dinglebear-ai/cortex:latest".to_string()),
        image_id: Some("sha256:abc".to_string()),
        compose_project: Some("cortex".to_string()),
        compose_working_dir: Some(PathBuf::from("/opt/cortex")),
        compose_files: vec![PathBuf::from("/opt/cortex/docker-compose.yml")],
        service: Some("cortex".to_string()),
        data_mounts: Vec::new(),
        ports: Vec::new(),
        systemd: None,
        diagnostics: vec![cortex::compose::ComposeDiagnostic {
            severity: cortex::compose::DiagnosticSeverity::Warning,
            code: "listener_missing".to_string(),
            message: "listener not observed".to_string(),
        }],
    };
    let coordination = vec![SetupPhase {
        name: "compose",
        status: SetupStatus::Skipped,
        detail: "dry run".to_string(),
    }];

    print_compose_status_response(&status, false).unwrap();
    print_compose_doctor_response(&status, &coordination, false).unwrap();
    ensure_doctor_coordination_ok(&coordination).unwrap();

    print_compose_command_response(
        &cortex::compose::ComposeCommandResult::DryRun(cortex::compose::ComposeDryRun {
            dry_run: true,
            command: vec![
                "docker".to_string(),
                "compose".to_string(),
                "up".to_string(),
            ],
            target: cortex::compose::ComposeTargetSummary {
                project_dir: Some(PathBuf::from("/opt/cortex")),
                compose_file: Some(PathBuf::from("/opt/cortex/docker-compose.yml")),
                project_name: Some("cortex".to_string()),
                service: "cortex".to_string(),
                container_name: "cortex".to_string(),
            },
            preflight: "ok".to_string(),
        }),
        false,
    )
    .unwrap();

    print_compose_command_response(
        &cortex::compose::ComposeCommandResult::Executed(cortex::compose::CommandOutput {
            exit_status: Some(0),
            stdout: "done\n".to_string(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: false,
            timeout_cleanup: None,
        }),
        false,
    )
    .unwrap();
}
