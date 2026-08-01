use super::*;
use crate::config::StorageConfig;
use crate::db::{
    ENTITY_TYPES, EVIDENCE_SOURCE_KINDS, LogBatchEntry, REASON_CODES, RELATIONSHIP_TYPES,
    TRUST_LEVELS, insert_logs_batch, is_known_entity_type, is_known_evidence_source_kind,
    is_known_reason_code, is_known_relationship_type, is_known_trust_level,
};

fn test_storage_config(db_path: std::path::PathBuf) -> StorageConfig {
    StorageConfig::for_test(db_path)
}

#[test]
fn test_init_pool_enables_incremental_auto_vacuum() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("autovac.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();
    let mode: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, 2);
}

#[test]
fn test_init_pool_migrates_existing_db_to_incremental_auto_vacuum() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("legacy.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA auto_vacuum=NONE;
         VACUUM;
         CREATE TABLE legacy_probe(id INTEGER PRIMARY KEY);",
    )
    .unwrap();
    drop(conn);

    let config = test_storage_config(db_path);
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();
    let mode: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, 2);
}

#[test]
fn test_init_pool_applies_busy_timeout_to_each_pooled_connection() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_storage_config(dir.path().join("busy-timeout.db"));
    config.pool_size = 2;
    let pool = init_pool(&config).unwrap();

    let conn1 = pool.get().unwrap();
    let conn2 = pool.get().unwrap();

    let busy_timeout_1: i64 = conn1
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    let busy_timeout_2: i64 = conn2
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();

    assert_eq!(busy_timeout_1, 5000);
    assert_eq!(busy_timeout_2, 5000);
}

#[test]
fn init_pool_applies_sqlite_cache_budget_to_each_pooled_connection() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_storage_config(dir.path().join("cache-budget.db"));
    config.pool_size = 2;
    config.sqlite_page_cache_mb = 128;

    let pool = init_pool(&config).unwrap();
    let conn1 = pool.get().unwrap();
    let conn2 = pool.get().unwrap();

    let cache_1: i64 = conn1
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .unwrap();
    let cache_2: i64 = conn2
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .unwrap();

    assert_eq!(cache_1, -65_536);
    assert_eq!(cache_2, -65_536);
}

#[test]
fn init_pool_applies_sqlite_mmap_to_each_pooled_connection() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_storage_config(dir.path().join("mmap.db"));
    config.pool_size = 2;
    config.sqlite_mmap_mb = 32;

    let pool = init_pool(&config).unwrap();
    let conn1 = pool.get().unwrap();
    let conn2 = pool.get().unwrap();

    let mmap_1: i64 = conn1
        .query_row("PRAGMA mmap_size", [], |row| row.get(0))
        .unwrap();
    let mmap_2: i64 = conn2
        .query_row("PRAGMA mmap_size", [], |row| row.get(0))
        .unwrap();

    assert_eq!(mmap_1, 32 * 1024 * 1024);
    assert_eq!(mmap_2, 32 * 1024 * 1024);
}

#[test]
fn init_db_creates_heartbeat_schema_migration_15() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("heartbeat.db");
    let config = test_storage_config(db_path);

    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    let applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 15",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied, 1);

    for table in [
        "host_heartbeats",
        "heartbeat_cpu",
        "heartbeat_memory",
        "heartbeat_disks",
        "heartbeat_network",
        "heartbeat_processes",
        "heartbeat_containers",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing heartbeat table {table}");
    }

    for index in [
        "idx_host_heartbeats_host_sampled",
        "idx_host_heartbeats_received",
        "idx_host_heartbeats_hostname_sampled",
        "idx_heartbeat_cpu_heartbeat_id",
        "idx_heartbeat_memory_heartbeat_id",
        "idx_heartbeat_disks_heartbeat_id",
        "idx_heartbeat_network_heartbeat_id",
        "idx_heartbeat_processes_heartbeat_id",
        "idx_heartbeat_containers_heartbeat_id",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [index],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing heartbeat index {index}");
    }
}

#[test]
fn init_db_creates_timeline_and_jobs_schema_migrations_25_26() {
    // Validate migrations 25 + 26 on a CLEAN temp DB (never touch prod).
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("mig25_26.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    for version in [25, 26] {
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                [version],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1, "migration {version} not recorded");
    }

    for table in [
        "timeline_hourly",
        "timeline_hourly_meta",
        "maintenance_jobs",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing table {table}");
    }

    // Meta row is seeded with watermark 0 / never-refreshed on a fresh DB.
    let (refreshed, max_id): (Option<String>, i64) = conn
        .query_row(
            "SELECT refreshed_at, source_max_id FROM timeline_hourly_meta WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(refreshed.is_none());
    assert_eq!(max_id, 0);

    // Empty DB => backfill skipped => rollup empty.
    let rollup_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM timeline_hourly", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rollup_rows, 0);
}

#[test]
fn init_db_creates_graph_schema_migration_27() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("graph.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    let applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 27",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied, 1, "migration 27 not recorded");

    for table in [
        "graph_entities",
        "graph_entity_aliases",
        "graph_relationships",
        "graph_relationship_evidence",
        "graph_projection_meta",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing graph table {table}");
    }

    let (status, degraded): (String, i64) = conn
        .query_row(
            "SELECT projection_status, is_degraded FROM graph_projection_meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "never_built");
    assert_eq!(degraded, 0);
}

#[test]
fn graph_migration_is_idempotent_and_preserves_raw_logs() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("graph-idempotent.db");
    let config = test_storage_config(db_path);
    let pool = init_pool(&config).unwrap();

    let inserted = insert_logs_batch(
        &pool,
        &[LogBatchEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            hostname: "claimed-host".to_string(),
            facility: None,
            severity: "info".to_string(),
            app_name: Some("sshd".to_string()),
            process_id: None,
            message: "accepted publickey".to_string(),
            raw: "accepted publickey".to_string(),
            source_ip: "10.0.0.1:514".to_string(),
            docker_checkpoint: None,
            ai_tool: None,
            ai_project: None,
            ai_session_id: None,
            ai_transcript_path: None,
            metadata_json: None,
            http_status: None,
            auth_outcome: None,
            dns_blocked: None,
            event_action: None,
            parse_error: None,
        }],
    )
    .unwrap();
    assert_eq!(inserted, 1);
    drop(pool);

    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();
    let log_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(log_count, 1, "graph migration must not mutate raw logs");

    let migration_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 27",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        migration_count, 1,
        "graph migration marker must remain idempotent"
    );
}

#[test]
fn graph_migration_converges_after_schema_exists_without_marker() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("graph-partial.db"));
    let pool = init_pool(&config).unwrap();
    {
        let conn = pool.get().unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 27", [])
            .unwrap();
    }
    drop(pool);

    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();
    let migration_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 27",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        migration_count, 1,
        "migration 27 must converge when DDL already exists"
    );
}

#[test]
fn known_schema_version_matches_migration_head() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("schema-head.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    let max_version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(max_version, KNOWN_SCHEMA_VERSION);
    drop(conn);

    let info = read_schema_version_info(&pool).unwrap();
    assert_eq!(info.version, KNOWN_SCHEMA_VERSION);
    assert_eq!(info.known_version, KNOWN_SCHEMA_VERSION);
}

#[test]
fn init_pool_creates_agent_observatory_repository_schema_scaffold() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("observatory-repositories.db"));

    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(repositories)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        columns,
        vec![
            "id",
            "repository_key",
            "hostname",
            "common_git_dir",
            "primary_path",
            "display_name",
            "remote_url_hash",
            "first_seen_at",
            "last_seen_at",
            "removed_at",
            "metadata_json",
            "created_at",
            "updated_at",
        ]
    );

    let indexes: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'index' AND tbl_name = 'repositories'
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_repositories_display")
    );
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_repositories_host_seen")
    );

    conn.execute(
        "INSERT INTO repositories
            (repository_key, hostname, common_git_dir, primary_path, display_name,
             first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        rusqlite::params![
            "v1|6:dookie|20:/workspace/cortex/.git",
            "dookie",
            "/workspace/cortex/.git",
            "/workspace/cortex",
            "cortex",
            "2026-07-31T23:00:00.000Z",
        ],
    )
    .unwrap();
    assert!(
        conn.execute(
            "INSERT INTO repositories
                (repository_key, hostname, common_git_dir, primary_path, display_name,
                 first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            rusqlite::params![
                "v1|6:dookie|20:/workspace/cortex/.git",
                "other-host",
                "/workspace/other/.git",
                "/workspace/other",
                "other",
                "2026-07-31T23:00:00.000Z",
            ],
        )
        .is_err(),
        "repository_key must be globally unique"
    );
    assert!(
        conn.execute(
            "INSERT INTO repositories
                (repository_key, hostname, common_git_dir, primary_path, display_name,
                 first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            rusqlite::params![
                "different-key",
                "dookie",
                "/workspace/cortex/.git",
                "/workspace/cortex-copy",
                "cortex-copy",
                "2026-07-31T23:00:00.000Z",
            ],
        )
        .is_err(),
        "hostname/common_git_dir must identify one repository"
    );
    drop(conn);
    drop(pool);

    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();
    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM repositories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(row_count, 1, "reopening must preserve repository rows");
    let migration_44_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 44",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        migration_44_count, 0,
        "migration 44 remains unmarked until its complete topology schema lands"
    );
}

#[test]
fn graph_schema_enforces_vocabulary_and_dedup_keys() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("graph-dedup.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES ('source_ip', '10.0.0.1:514', '10.0.0.1:514', 'verified')",
        [],
    )
    .unwrap();
    let duplicate = conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES ('source_ip', '10.0.0.1:514', 'duplicate', 'verified')",
        [],
    );
    assert!(duplicate.is_err(), "canonical entity identity must dedupe");

    let bad_type = conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES ('same_window', 'bad', 'bad', 'verified')",
        [],
    );
    assert!(bad_type.is_err(), "unknown entity types must be rejected");

    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, trust_level)
         VALUES ('host', 'claimed-host', 'claimed-host', 'claimed')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES ('reverse_proxy', 'proxy:example.tootie.tv', 'example.tootie.tv',
             'app_inventory', 'proxy:example.tootie.tv', 'verified')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES ('domain', 'example.tootie.tv', 'example.tootie.tv',
             'app_inventory', 'example.tootie.tv', 'verified')",
        [],
    )
    .unwrap();
    let source_id: i64 = conn
        .query_row(
            "SELECT id FROM graph_entities WHERE entity_type = 'source_ip'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let host_id: i64 = conn
        .query_row(
            "SELECT id FROM graph_entities WHERE entity_type = 'host'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let proxy_id: i64 = conn
        .query_row(
            "SELECT id FROM graph_entities WHERE entity_type = 'reverse_proxy'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let domain_id: i64 = conn
        .query_row(
            "SELECT id FROM graph_entities WHERE entity_type = 'domain'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    conn.execute(
        "INSERT INTO graph_entity_aliases
            (entity_id, alias_type, alias_key, alias_value, source_kind, trust_level)
         VALUES (?1, 'hostname', 'claimed-host', 'claimed-host', 'log', 'claimed')",
        [host_id],
    )
    .unwrap();
    let duplicate_alias = conn.execute(
        "INSERT INTO graph_entity_aliases
            (entity_id, alias_type, alias_key, alias_value, source_kind, trust_level)
         VALUES (?1, 'hostname', 'claimed-host', 'claimed-host', 'log', 'claimed')",
        [host_id],
    );
    assert!(duplicate_alias.is_err(), "alias identity must dedupe");

    conn.execute(
        "INSERT INTO graph_relationships
            (relationship_key, src_entity_id, dst_entity_id, relationship_type,
             reason_code, trust_level, confidence, evidence_count)
         VALUES ('source_ip:10.0.0.1:514->host:claimed-host', ?1, ?2,
             'observed_as', 'syslog_claimed_hostname', 'claimed', 0.60, 1)",
        rusqlite::params![source_id, host_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO graph_relationships
            (relationship_key, src_entity_id, dst_entity_id, relationship_type,
             reason_code, trust_level, confidence, evidence_count)
         VALUES ('reverse_proxy:example.tootie.tv->domain:example.tootie.tv',
             ?1, ?2, 'exposes_domain', 'reverse_proxy_config',
             'verified', 0.90, 1)",
        rusqlite::params![proxy_id, domain_id],
    )
    .unwrap();
    let duplicate_rel = conn.execute(
        "INSERT INTO graph_relationships
            (relationship_key, src_entity_id, dst_entity_id, relationship_type,
             reason_code, trust_level, confidence, evidence_count)
         VALUES ('source_ip:10.0.0.1:514->host:claimed-host', ?1, ?2,
             'observed_as', 'syslog_claimed_hostname', 'claimed', 0.60, 1)",
        rusqlite::params![source_id, host_id],
    );
    assert!(duplicate_rel.is_err(), "relationship key must dedupe");

    let rel_id: i64 = conn
        .query_row(
            "SELECT id FROM graph_relationships
             WHERE relationship_key = 'source_ip:10.0.0.1:514->host:claimed-host'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO graph_relationship_evidence
            (relationship_id, evidence_key, source_kind, source_id, observed_at,
             reason_code, trust_level, safe_excerpt, evidence_count)
         VALUES (?1, 'log:1:hostname:2026-01-01T00', 'log', '1',
             '2026-01-01T00:00:00Z', 'syslog_claimed_hostname',
             'claimed', 'claimed-host', 3)",
        [rel_id],
    )
    .unwrap();
    let proxy_rel_id: i64 = conn
        .query_row(
            "SELECT id FROM graph_relationships
             WHERE relationship_key = 'reverse_proxy:example.tootie.tv->domain:example.tootie.tv'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO graph_relationship_evidence
            (relationship_id, evidence_key, source_kind, source_id, observed_at,
             reason_code, trust_level, safe_excerpt, evidence_count)
         VALUES (?1, 'proxy:example.tootie.tv:route',
             'app_inventory', 'proxy:example.tootie.tv',
             '2026-01-01T00:00:00Z', 'reverse_proxy_config',
             'verified', 'example.tootie.tv routes through proxy config', 1)",
        [proxy_rel_id],
    )
    .unwrap();
    let duplicate_evidence = conn.execute(
        "INSERT INTO graph_relationship_evidence
            (relationship_id, evidence_key, source_kind, source_id, observed_at,
             reason_code, trust_level, safe_excerpt, evidence_count)
         VALUES (?1, 'log:1:hostname:2026-01-01T00', 'log', '1',
             '2026-01-01T00:00:00Z', 'syslog_claimed_hostname',
             'claimed', 'claimed-host', 3)",
        [rel_id],
    );
    assert!(
        duplicate_evidence.is_err(),
        "evidence key must dedupe repeated samples"
    );

    let bad_same_window = conn.execute(
        "INSERT INTO graph_relationships
            (relationship_key, src_entity_id, dst_entity_id, relationship_type,
             reason_code, trust_level)
         VALUES ('bad-same-window', ?1, ?2, 'same_window',
             'syslog_claimed_hostname', 'correlated')",
        rusqlite::params![source_id, host_id],
    );
    assert!(
        bad_same_window.is_err(),
        "same_window must not be a persisted v1 relationship type"
    );
}

#[test]
fn migration_30_widens_old_graph_constraints_and_preserves_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("graph-migration-30.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             );
             WITH RECURSIVE versions(version) AS (
                 SELECT 1 UNION ALL SELECT version + 1 FROM versions WHERE version < 29
             )
             INSERT INTO schema_migrations(version) SELECT version FROM versions;
             CREATE TABLE maintenance_jobs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL,
                 status TEXT NOT NULL,
                 started_at TEXT NOT NULL,
                 finished_at TEXT,
                 result_json TEXT
             );
             CREATE TABLE graph_entities (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 entity_type TEXT NOT NULL CHECK (entity_type IN (
                     'host', 'container', 'service', 'app', 'source_ip',
                     'ai_project', 'ai_session', 'error_signature'
                 )),
                 canonical_key TEXT NOT NULL,
                 display_label TEXT NOT NULL,
                 source_kind TEXT NOT NULL DEFAULT '',
                 source_id TEXT NOT NULL DEFAULT '',
                 trust_level TEXT NOT NULL CHECK (trust_level IN (
                     'verified', 'claimed', 'inferred', 'correlated'
                 )),
                 first_seen_at TEXT,
                 last_seen_at TEXT,
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 UNIQUE(entity_type, canonical_key)
             );
             CREATE TABLE graph_entity_aliases (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 entity_id INTEGER NOT NULL,
                 alias_type TEXT NOT NULL,
                 alias_key TEXT NOT NULL,
                 alias_value TEXT NOT NULL,
                 source_kind TEXT NOT NULL DEFAULT '',
                 trust_level TEXT NOT NULL CHECK (trust_level IN (
                     'verified', 'claimed', 'inferred', 'correlated'
                 )),
                 first_seen_at TEXT,
                 last_seen_at TEXT,
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 UNIQUE(entity_id, alias_type, alias_key, source_kind)
             );
             CREATE TABLE graph_relationships (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 relationship_key TEXT NOT NULL UNIQUE,
                 src_entity_id INTEGER NOT NULL,
                 dst_entity_id INTEGER NOT NULL,
                 relationship_type TEXT NOT NULL CHECK (relationship_type IN (
                     'observed_as', 'runs_on', 'emitted_by', 'worked_on',
                     'matches_signature'
                 )),
                 reason_code TEXT NOT NULL CHECK (reason_code IN (
                     'syslog_claimed_hostname', 'log_app_name',
                     'docker_container_id', 'docker_service_label',
                     'ai_session_project', 'heartbeat_host_state',
                     'error_signature_match'
                 )),
                 trust_level TEXT NOT NULL CHECK (trust_level IN (
                     'verified', 'claimed', 'inferred', 'correlated'
                 )),
                 confidence REAL NOT NULL DEFAULT 0.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
                 evidence_count INTEGER NOT NULL DEFAULT 0 CHECK (evidence_count >= 0),
                 first_seen_at TEXT,
                 last_seen_at TEXT,
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 UNIQUE(src_entity_id, dst_entity_id, relationship_type, relationship_key)
             );
             CREATE TABLE graph_relationship_evidence (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 relationship_id INTEGER NOT NULL,
                 evidence_key TEXT NOT NULL,
                 source_kind TEXT NOT NULL CHECK (source_kind IN (
                     'log', 'heartbeat', 'ai_session_rollup', 'error_signature'
                 )),
                 source_id TEXT NOT NULL DEFAULT '',
                 source_log_id INTEGER,
                 source_heartbeat_id INTEGER,
                 source_signature_hash TEXT,
                 observed_at TEXT NOT NULL,
                 reason_code TEXT NOT NULL CHECK (reason_code IN (
                     'syslog_claimed_hostname', 'log_app_name',
                     'docker_container_id', 'docker_service_label',
                     'ai_session_project', 'heartbeat_host_state',
                     'error_signature_match'
                 )),
                 reason_text TEXT,
                 confidence_delta REAL NOT NULL DEFAULT 0.0 CHECK (confidence_delta >= 0.0 AND confidence_delta <= 1.0),
                 trust_level TEXT NOT NULL CHECK (trust_level IN (
                     'verified', 'claimed', 'inferred', 'correlated'
                 )),
                 safe_excerpt TEXT,
                 metadata_path TEXT,
                 evidence_count INTEGER NOT NULL DEFAULT 1 CHECK (evidence_count >= 1),
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 UNIQUE(relationship_id, evidence_key)
             );
             CREATE TABLE graph_projection_meta (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 projection_status TEXT NOT NULL DEFAULT 'pending',
                 last_started_at TEXT,
                 last_completed_at TEXT,
                 source_watermark TEXT NOT NULL DEFAULT '',
                 source_row_count INTEGER NOT NULL DEFAULT 0 CHECK (source_row_count >= 0),
                 entity_count INTEGER NOT NULL DEFAULT 0 CHECK (entity_count >= 0),
                 relationship_count INTEGER NOT NULL DEFAULT 0 CHECK (relationship_count >= 0),
                 evidence_count INTEGER NOT NULL DEFAULT 0 CHECK (evidence_count >= 0),
                 is_degraded INTEGER NOT NULL DEFAULT 0 CHECK (is_degraded IN (0, 1)),
                 last_error TEXT,
                 last_runtime_ms INTEGER NOT NULL DEFAULT 0 CHECK (last_runtime_ms >= 0),
                 last_chunk_count INTEGER NOT NULL DEFAULT 0 CHECK (last_chunk_count >= 0),
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             );
             INSERT INTO graph_projection_meta(id) VALUES (1);
             INSERT INTO graph_entities
                 (id, entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
             VALUES
                 (1, 'source_ip', '10.0.0.1:514', '10.0.0.1:514', 'log', '1', 'verified'),
                 (2, 'host', 'claimed-host', 'claimed-host', 'log', '1', 'claimed');
             INSERT INTO graph_entity_aliases
                 (id, entity_id, alias_type, alias_key, alias_value, source_kind, trust_level)
             VALUES (1, 2, 'hostname', 'claimed-host', 'claimed-host', 'log', 'claimed');
             INSERT INTO graph_relationships
                 (id, relationship_key, src_entity_id, dst_entity_id, relationship_type,
                  reason_code, trust_level, confidence, evidence_count)
             VALUES (1, 'source_ip:10.0.0.1:514->host:claimed-host', 1, 2,
                 'observed_as', 'syslog_claimed_hostname', 'claimed', 0.60, 1);
             INSERT INTO graph_relationship_evidence
                 (id, relationship_id, evidence_key, source_kind, source_id, observed_at,
                  reason_code, trust_level, safe_excerpt, evidence_count)
             VALUES (1, 1, 'log:1:hostname', 'log', '1', '2026-01-01T00:00:00Z',
                 'syslog_claimed_hostname', 'claimed', 'claimed-host', 1);",
        )
        .unwrap();
    }

    let pool = init_pool(&test_storage_config(db_path)).unwrap();
    let conn = pool.get().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM graph_relationship_evidence WHERE evidence_key = 'log:1:hostname'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES ('compose_project', 'squirts:edge', 'edge',
             'app_inventory', 'compose:squirts', 'verified')",
        [],
    )
    .unwrap();
    let relationship_id = conn
        .query_row(
            "SELECT id FROM graph_relationships
              WHERE relationship_key = 'source_ip:10.0.0.1:514->host:claimed-host'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO graph_relationship_evidence
            (relationship_id, evidence_key, source_kind, source_id, observed_at,
             reason_code, trust_level, safe_excerpt, evidence_count)
         VALUES (?1, 'inventory:route', 'app_inventory', 'proxy:squirts',
             '2026-01-01T00:00:00Z', 'reverse_proxy_config',
             'verified', 'proxy route', 1)",
        rusqlite::params![relationship_id],
    )
    .unwrap();
}

#[test]
fn graph_vocabulary_helpers_cover_schema_values() {
    for value in ENTITY_TYPES {
        assert!(is_known_entity_type(value), "missing entity type {value}");
    }
    for value in RELATIONSHIP_TYPES {
        assert!(
            is_known_relationship_type(value),
            "missing relationship type {value}"
        );
    }
    for value in REASON_CODES {
        assert!(is_known_reason_code(value), "missing reason code {value}");
    }
    for value in TRUST_LEVELS {
        assert!(is_known_trust_level(value), "missing trust level {value}");
    }
    for value in EVIDENCE_SOURCE_KINDS {
        assert!(
            is_known_evidence_source_kind(value),
            "missing evidence source kind {value}"
        );
    }

    assert!(!is_known_relationship_type("same_window"));
    assert!(!is_known_entity_type("unknown"));
    assert!(!is_known_evidence_source_kind("source_table"));
}

#[test]
fn graph_lookup_indexes_support_expected_query_plans() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("graph-query-plan.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    let plan_details = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };

    let entity_plan = plan_details(
        "EXPLAIN QUERY PLAN
         SELECT id FROM graph_entities
         WHERE entity_type = 'host' AND canonical_key = 'dookie'",
    );
    assert!(
        entity_plan
            .iter()
            .any(|p| p.contains("SEARCH graph_entities")),
        "entity lookup must use an indexed search: {entity_plan:?}"
    );

    let alias_plan = plan_details(
        "EXPLAIN QUERY PLAN
         SELECT entity_id FROM graph_entity_aliases
         WHERE alias_type = 'hostname' AND alias_key = 'dookie'",
    );
    assert!(
        alias_plan
            .iter()
            .any(|p| p.contains("SEARCH graph_entity_aliases")),
        "alias lookup must use an indexed search: {alias_plan:?}"
    );

    let outgoing_plan = plan_details(
        "EXPLAIN QUERY PLAN
         SELECT id FROM graph_relationships
         WHERE src_entity_id = 1 AND relationship_type = 'observed_as'
         ORDER BY last_seen_at DESC LIMIT 50",
    );
    assert!(
        outgoing_plan
            .iter()
            .any(|p| p.contains("SEARCH graph_relationships")),
        "outgoing relationship lookup must use an indexed search: {outgoing_plan:?}"
    );
    assert!(
        !outgoing_plan
            .iter()
            .any(|p| p == "SCAN graph_relationships"),
        "outgoing relationship lookup must not full-scan relationship table: {outgoing_plan:?}"
    );

    let incoming_plan = plan_details(
        "EXPLAIN QUERY PLAN
         SELECT id FROM graph_relationships
         WHERE dst_entity_id = 2 AND relationship_type = 'observed_as'
         ORDER BY last_seen_at DESC LIMIT 50",
    );
    assert!(
        incoming_plan
            .iter()
            .any(|p| p.contains("SEARCH graph_relationships")),
        "incoming relationship lookup must use an indexed search: {incoming_plan:?}"
    );
    assert!(
        !incoming_plan
            .iter()
            .any(|p| p == "SCAN graph_relationships"),
        "incoming relationship lookup must not full-scan relationship table: {incoming_plan:?}"
    );

    let evidence_plan = plan_details(
        "EXPLAIN QUERY PLAN
         SELECT id FROM graph_relationship_evidence
         WHERE relationship_id = 1
         ORDER BY observed_at DESC LIMIT 3",
    );
    assert!(
        evidence_plan
            .iter()
            .any(|p| p.contains("SEARCH graph_relationship_evidence")),
        "evidence lookup must use an indexed search: {evidence_plan:?}"
    );
    assert!(
        !evidence_plan
            .iter()
            .any(|p| p == "SCAN graph_relationship_evidence"),
        "evidence lookup must not full-scan evidence table: {evidence_plan:?}"
    );

    let source_cleanup_plan = plan_details(
        "EXPLAIN QUERY PLAN
         SELECT id FROM graph_relationship_evidence
         WHERE source_kind = 'log' AND source_id = '1'",
    );
    assert!(
        source_cleanup_plan
            .iter()
            .any(|p| p.contains("SEARCH graph_relationship_evidence")),
        "source cleanup lookup must use an indexed search: {source_cleanup_plan:?}"
    );
}

#[test]
fn heartbeat_schema_enforces_idempotency_key() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("heartbeat-unique.db");
    let config = test_storage_config(db_path);

    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    let insert = "INSERT INTO host_heartbeats (
        host_id, hostname, source_ip, sampled_at, received_at, boot_id,
        uptime_secs, sequence, collection_ms, partial, agent_version, os, architecture
    ) VALUES (
        'host-1', 'box-a', '127.0.0.1:3100', '2026-05-25T00:00:00Z',
        '2026-05-25T00:00:01Z', 'boot-a', 60, 1, 12, 0, '0.1.0', 'linux', 'x86_64'
    )";
    conn.execute(insert, []).unwrap();
    let duplicate = conn.execute(insert, []);
    assert!(
        duplicate.is_err(),
        "duplicate heartbeat key must be rejected"
    );
}

#[test]
fn init_db_adds_ai_session_metadata_columns() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = crate::config::StorageConfig {
        db_path,
        ..Default::default()
    };

    let _pool = init_pool(&config).unwrap();
    let conn = rusqlite::Connection::open(&config.db_path).unwrap();
    for column in [
        "ai_tool",
        "ai_project",
        "ai_session_id",
        "ai_transcript_path",
        "metadata_json",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('logs') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing column {column}");
    }
}

#[test]
fn init_db_creates_partial_ai_metadata_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = crate::config::StorageConfig {
        db_path,
        ..Default::default()
    };

    let _pool = init_pool(&config).unwrap();
    let conn = rusqlite::Connection::open(&config.db_path).unwrap();
    let indexes: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT name, sql FROM sqlite_schema
                 WHERE type = 'index'
                   AND name IN (
                     'idx_logs_ai_project_time',
                     'idx_logs_ai_session',
                     'idx_logs_ai_transcript_path'
                   )
                 ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };

    assert_eq!(indexes.len(), 3);
    for (_, sql) in indexes {
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("IS NOT NULL"));
    }
}

#[test]
fn migrations_23_24_yield_final_covering_index_set() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = crate::config::StorageConfig {
        db_path,
        ..Default::default()
    };

    let _pool = init_pool(&config).unwrap();
    let conn = rusqlite::Connection::open(&config.db_path).unwrap();

    let index_sql = |name: &str| -> Option<String> {
        conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = ?1",
            [name],
            |row| row.get::<_, String>(0),
        )
        .ok()
    };

    // Migration 23's interim AI index is superseded and DROPped by migration 24.
    assert!(
        index_sql("idx_logs_ai_project_cover").is_none(),
        "migration 24 must drop the superseded idx_logs_ai_project_cover"
    );

    // errors covering index (migration 23) survives.
    let sev_cover = index_sql("idx_logs_sev_host_time").expect("severity/host covering index");
    assert!(sev_cover.contains("severity"));
    assert!(sev_cover.contains("hostname"));
    assert!(sev_cover.contains("timestamp"));

    // Timestamp-positioned AI covering index (migration 24) serves ai projects + ai blocks.
    let ts_cover = index_sql("idx_logs_ai_project_ts_cover").expect("ai project ts-covering index");
    // Column order matters: ai_project, THEN timestamp (seekable), then the covered cols.
    let p = ts_cover.find("ai_project").unwrap();
    let t = ts_cover.find("timestamp").unwrap();
    let tool = ts_cover.find("ai_tool").unwrap();
    assert!(
        p < t && t < tool,
        "order must be ai_project, timestamp, ai_tool, ..."
    );
    assert!(ts_cover.contains("ai_session_id"));
    assert!(ts_cover.contains("ai_project IS NOT NULL"));

    // ai tools covering index (migration 24).
    let tool_cover = index_sql("idx_logs_ai_tool_cover").expect("ai tool covering index");
    assert!(tool_cover.contains("ai_tool"));
    assert!(tool_cover.contains("ai_session_id"));
    assert!(tool_cover.contains("timestamp"));

    // Migration 24 only ANALYZEs when `logs` already has rows, so this empty
    // fresh DB writes no `sqlite_stat1` (by design — empty-table stats mislead
    // the planner). The populated-DB ANALYZE path is covered by live validation.

    for v in [23, 24] {
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                [v],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1, "migration {v} must be recorded");
    }
}

#[test]
fn migration_32_covers_graph_to_log_join() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("graph-log-cover.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    // Index DDL is present and carries the expected column order.
    let index_sql = |name: &str| -> Option<String> {
        conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = ?1",
            [name],
            |row| row.get::<_, String>(0),
        )
        .ok()
    };

    let cover = index_sql("idx_logs_hostname_appname_time")
        .expect("graph→log covering index must exist after migration 32");
    let h = cover.find("hostname").unwrap();
    let a = cover.find("app_name").unwrap();
    let t = cover.find("timestamp").unwrap();
    assert!(
        h < a && a < t,
        "column order must be hostname, app_name, timestamp: {cover}"
    );

    let session_cover = index_sql("idx_logs_ai_session_time")
        .expect("session-anchored covering index must exist after migration 32");
    assert!(session_cover.contains("ai_session_id"));
    assert!(session_cover.contains("timestamp"));
    assert!(session_cover.contains("ai_session_id IS NOT NULL"));

    // The planner must pick the covering index for the topic_correlate join shape:
    // hostname IN (...) AND timestamp BETWEEN ... AND app_name = ...
    let plan_details = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let join_plan = plan_details(
        "EXPLAIN QUERY PLAN
         SELECT id FROM logs
         WHERE hostname IN ('dookie', 'squirts')
           AND app_name = 'swag'
           AND timestamp BETWEEN '2026-06-18T00:00:00Z' AND '2026-06-18T01:00:00Z'",
    );
    assert!(
        join_plan
            .iter()
            .any(|p| p.contains("idx_logs_hostname_appname_time")),
        "graph→log join must use idx_logs_hostname_appname_time: {join_plan:?}"
    );

    let applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 32",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied, 1, "migration 32 must be recorded");
}

#[test]
fn init_db_creates_inventory_stats_tables_and_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = crate::config::StorageConfig {
        db_path,
        ..Default::default()
    };

    let _pool = init_pool(&config).unwrap();
    let conn = rusqlite::Connection::open(&config.db_path).unwrap();
    for table in [
        "app_inventory_stats",
        "app_host_inventory_stats",
        "source_ip_inventory_stats",
        "source_ip_host_inventory_stats",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing table {table}");
    }
    for trigger in [
        "logs_inventory_app_ai",
        "logs_inventory_app_ad",
        "logs_inventory_source_ip_ai",
        "logs_inventory_source_ip_ad",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing trigger {trigger}");
    }
}

#[test]
fn inventory_backfill_processes_existing_logs_in_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig {
        db_path: dir.path().join("test.db"),
        ..Default::default()
    };
    let pool = init_pool(&config).unwrap();
    let mut entries = Vec::new();
    for i in 0..3 {
        entries.push(LogBatchEntry {
            timestamp: format!("2026-01-01T00:00:0{i}Z"),
            hostname: format!("host-{i}"),
            facility: None,
            severity: "info".to_string(),
            app_name: Some("nginx".to_string()),
            process_id: None,
            message: "hello".to_string(),
            raw: "hello".to_string(),
            source_ip: "10.0.0.1:514".to_string(),
            docker_checkpoint: None,
            ai_tool: None,
            ai_project: None,
            ai_session_id: None,
            ai_transcript_path: None,
            metadata_json: None,
            http_status: None,
            auth_outcome: None,
            dns_blocked: None,
            event_action: None,
            parse_error: None,
        });
    }
    insert_logs_batch(&pool, &entries).unwrap();

    let conn = pool.get().unwrap();
    conn.execute("DELETE FROM app_inventory_stats", []).unwrap();
    conn.execute("DELETE FROM app_host_inventory_stats", [])
        .unwrap();
    conn.execute("DELETE FROM source_ip_inventory_stats", [])
        .unwrap();
    conn.execute("DELETE FROM source_ip_host_inventory_stats", [])
        .unwrap();
    drop(conn);

    backfill_inventory_stats(&pool).unwrap();

    let conn = pool.get().unwrap();
    let complete: bool = conn
        .query_row(
            "SELECT completed_at IS NOT NULL
             FROM inventory_backfill_state
             WHERE name = 'app_source_inventory'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(complete);
    let app_count: i64 = conn
        .query_row(
            "SELECT log_count FROM app_inventory_stats WHERE app_name = 'nginx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(app_count, 3);
    let source_count: i64 = conn
        .query_row(
            "SELECT log_count FROM source_ip_inventory_stats WHERE source_ip = '10.0.0.1:514'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_count, 3);
}

#[test]
fn init_db_adds_transcript_checkpoint_tables() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = crate::config::StorageConfig {
        db_path,
        ..Default::default()
    };

    let _pool = init_pool(&config).unwrap();
    let conn = rusqlite::Connection::open(&config.db_path).unwrap();
    for table in [
        "transcript_sources",
        "transcript_import_records",
        "transcript_parse_errors",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing table {table}");
    }
    let preview_not_null: i64 = conn
        .query_row(
            "SELECT [notnull] FROM pragma_table_info('transcript_parse_errors') WHERE name = 'record_preview'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preview_not_null, 1);
}

#[test]
fn init_db_migrates_legacy_ai_schema_without_losing_logs() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("legacy-ai.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE logs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp   TEXT NOT NULL,
            hostname    TEXT NOT NULL,
            facility    TEXT,
            severity    TEXT NOT NULL,
            app_name    TEXT,
            process_id  TEXT,
            message     TEXT NOT NULL,
            raw         TEXT NOT NULL,
            received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            source_ip   TEXT NOT NULL DEFAULT ''
        );
        CREATE VIRTUAL TABLE logs_fts USING fts5(
            message,
            content='logs',
            content_rowid='id',
            tokenize='porter unicode61'
        );
        CREATE TABLE hosts (
            hostname    TEXT PRIMARY KEY,
            first_seen  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            last_seen   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            log_count   INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE schema_migrations (
            version     INTEGER PRIMARY KEY,
            applied_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        INSERT INTO schema_migrations(version) VALUES (1), (2), (3);
        INSERT INTO logs(timestamp, hostname, facility, severity, app_name, process_id, message, raw, source_ip)
        VALUES ('2026-05-11T00:00:00Z', 'legacy-host', 'local0', 'info', 'legacy', NULL, 'legacy preserved', 'legacy preserved', '127.0.0.1:514');
        INSERT INTO logs_fts(rowid, message) VALUES (1, 'legacy preserved');
        INSERT INTO hosts(hostname, log_count) VALUES ('legacy-host', 1);
        ",
    )
    .unwrap();
    drop(conn);

    let pool = init_pool(&test_storage_config(db_path)).unwrap();
    let conn = pool.get().unwrap();
    for column in [
        "ai_tool",
        "ai_project",
        "ai_session_id",
        "ai_transcript_path",
        "metadata_json",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('logs') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing migrated column {column}");
    }
    for version in [4, 5, 6] {
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                [version],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1, "missing migration {version}");
    }
    let preserved: String = conn
        .query_row(
            "SELECT message FROM logs WHERE hostname = 'legacy-host'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preserved, "legacy preserved");
}

#[test]
fn migration_13_adds_enrichment_columns() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig {
        db_path: dir.path().join("test.db"),
        wal_mode: true,
        pool_size: 1,
        ..Default::default()
    };
    let pool = init_pool(&config).expect("init_pool ok");
    let conn = pool.get().unwrap();

    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(logs)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    for expected in [
        "http_status",
        "auth_outcome",
        "dns_blocked",
        "event_action",
        "parse_error",
    ] {
        assert!(
            cols.contains(&expected.to_string()),
            "missing column {expected}"
        );
    }

    let indices: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='logs'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();

    for expected in [
        "idx_logs_http_status_time",
        "idx_logs_auth_outcome_time",
        "idx_logs_dns_blocked_time",
        "idx_logs_event_action_time",
    ] {
        assert!(
            indices.contains(&expected.to_string()),
            "missing index {expected}"
        );
    }

    let version_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 13",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version_count, 1, "migration 13 row not recorded");
}

#[test]
fn migration_13_tolerates_existing_columns_without_version_row() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("migration-13-drift.db");
    let config = crate::config::StorageConfig {
        db_path: db_path.clone(),
        wal_mode: true,
        pool_size: 1,
        ..Default::default()
    };
    let pool = init_pool(&config).expect("initial init_pool ok");
    drop(pool);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM schema_migrations WHERE version = 13", [])
        .unwrap();
    conn.execute("DROP INDEX idx_logs_event_action_time", [])
        .unwrap();
    drop(conn);

    let pool = init_pool(&config).expect("re-init should repair migration drift");
    let conn = pool.get().unwrap();
    let version_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 13",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version_count, 1, "migration 13 row not restored");

    let index_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_logs_event_action_time'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index_count, 1, "migration 13 index not restored");
}

#[test]
fn transcript_import_identity_enforces_uniqueness() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::StorageConfig {
        db_path: dir.path().join("test.db"),
        ..Default::default()
    };

    let _pool = init_pool(&config).unwrap();
    let conn = rusqlite::Connection::open(&config.db_path).unwrap();
    conn.execute(
        "INSERT INTO transcript_sources (canonical_path, source_kind) VALUES (?1, ?2)",
        rusqlite::params!["/tmp/session.jsonl", "explicit_file"],
    )
    .unwrap();
    let source_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO transcript_import_records (source_id, record_key) VALUES (?1, ?2)",
        rusqlite::params![source_id, "record-1"],
    )
    .unwrap();
    let err = conn
        .execute(
            "INSERT INTO transcript_import_records (source_id, record_key) VALUES (?1, ?2)",
            rusqlite::params![source_id, "record-1"],
        )
        .unwrap_err();
    assert!(matches!(err, rusqlite::Error::SqliteFailure(_, _)));
}

/// Reproduces the post-crash state of Migration 22 (bead syslog-mcp-tfr0): a
/// crash between the `ALTER TABLE ... ADD COLUMN` statements and the version
/// marker leaves the watermark columns present but version 22 absent from
/// `schema_migrations`. We reach that identical on-disk state cheaply by
/// migrating clean to head, then deleting only the version-22 marker row.
///
/// On the pre-fix (bare `execute_batch`) code this FAILS: re-running `init_pool`
/// re-issues the unguarded ALTERs and aborts with "duplicate column name". The
/// Style-C rewrite guards each ALTER with `add_column_if_missing` and stamps the
/// version with `INSERT OR IGNORE`, so `init_pool` converges (reentrant) and the
/// partial state becomes crash-impossible (a real mid-tx crash now rolls back
/// both columns and the marker atomically).
#[test]
fn migration_22_converges_from_partial_apply() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("partial_m22.db");
    let config = test_storage_config(db_path.clone());

    // 1. Migrate a clean DB to head (version 22, both columns present).
    let pool = init_pool(&config).unwrap();
    {
        let conn = pool.get().unwrap();
        // Sanity: migration 22 specifically is applied, with the columns present.
        // Assert on version 22 directly (not MAX(version)) so a future migration 23
        // cannot break this test even though migration 22 is correctly applied.
        let m22_applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 22",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(m22_applied, 1, "fixture must reach migration 22");
        for column in ["source_row_count", "source_max_id"] {
            assert!(
                column_exists(&conn, "ai_session_rollup_meta", column).unwrap(),
                "fixture must have column {column}"
            );
        }
        // 2. Recreate the post-crash state: columns present, marker absent.
        conn.execute("DELETE FROM schema_migrations WHERE version = 22", [])
            .unwrap();
    }
    drop(pool); // release the pooled connections / file handles

    // 3. Re-running init_pool must converge, not brick on "duplicate column name".
    let pool =
        init_pool(&config).expect("init_pool must be reentrant after a partial migration 22 apply");
    let conn = pool.get().unwrap();

    // Assert migration 22 specifically was re-stamped (not MAX(version)) so a
    // future migration 23 cannot mask a missing 22 marker / break this test.
    let m22_applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 22",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(m22_applied, 1, "version marker must be re-stamped to 22");

    for column in ["source_row_count", "source_max_id"] {
        assert!(
            column_exists(&conn, "ai_session_rollup_meta", column).unwrap(),
            "watermark column {column} must remain present after convergence"
        );
    }
}

/// Regression guard (bead syslog-mcp-tfr0): running `init_pool` twice against the
/// same file must both succeed. This passes on the pre-fix code too — it is NOT
/// the bug-prover (`migration_22_converges_from_partial_apply` is) — it just pins
/// the idempotent-on-clean-reopen behaviour so a future migration change can't
/// silently break it.
#[test]
fn init_pool_is_idempotent_when_run_twice() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("idempotent.db");
    let config = test_storage_config(db_path);

    let pool = init_pool(&config).expect("first init_pool must succeed");
    drop(pool);

    let pool = init_pool(&config).expect("second init_pool on same file must succeed");
    let conn = pool.get().unwrap();
    // Assert migration 22 specifically is applied (not MAX(version)) so a future
    // migration 23 cannot break this test even though 22 is correctly applied.
    let m22_applied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 22",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(m22_applied, 1);
}

#[test]
fn migration_18_re_stamps_when_restarting_column_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("partial-m18.db");
    let config = test_storage_config(db_path.clone());

    let pool = init_pool(&config).unwrap();
    {
        let conn = pool.get().unwrap();
        assert!(
            column_exists(&conn, "heartbeat_containers", "restarting").unwrap(),
            "fixture must have heartbeat_containers.restarting"
        );
        conn.execute("DELETE FROM schema_migrations WHERE version = 18", [])
            .unwrap();
    }
    drop(pool);

    let pool = init_pool(&config).expect("migration 18 must converge with existing column");
    let conn = pool.get().unwrap();
    assert!(
        column_exists(&conn, "heartbeat_containers", "restarting").unwrap(),
        "restarting column must remain present"
    );
    let version_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 18",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version_count, 1, "migration 18 marker must be restored");
}

#[test]
fn migration_28_repairs_missing_runtime_metric_column_without_duplicate_marker() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("partial-m28.db");
    let config = test_storage_config(db_path.clone());

    let pool = init_pool(&config).unwrap();
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE graph_projection_meta
                SET last_runtime_ms = 4242,
                    last_chunk_count = 7
              WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 28", [])
            .unwrap();
        conn.execute(
            "ALTER TABLE graph_projection_meta DROP COLUMN last_chunk_count",
            [],
        )
        .unwrap();
    }
    drop(pool);

    let pool = init_pool(&config).expect("migration 28 must repair a missing runtime column");
    let conn = pool.get().unwrap();
    assert!(
        column_exists(&conn, "graph_projection_meta", "last_runtime_ms").unwrap(),
        "existing metric column must remain present"
    );
    assert!(
        column_exists(&conn, "graph_projection_meta", "last_chunk_count").unwrap(),
        "missing metric column must be restored"
    );
    let version_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 28",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        version_count, 1,
        "migration 28 marker must be restored exactly once"
    );
    let runtime_ms: i64 = conn
        .query_row(
            "SELECT last_runtime_ms FROM graph_projection_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runtime_ms, 4242, "existing metric data must survive repair");
}

/// Golden old-schema fixture: the exact v0.2.6 schema (pre-migration-framework
/// — no schema_migrations table, no ai_* columns, no metadata_json). Frozen
/// from `git show v0.2.6:src/db.rs`; do not "modernize" it — its purpose is to
/// represent a real old installation.
const V0_2_6_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS logs (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp   TEXT NOT NULL,
        hostname    TEXT NOT NULL,
        facility    TEXT,
        severity    TEXT NOT NULL,
        app_name    TEXT,
        process_id  TEXT,
        message     TEXT NOT NULL,
        raw         TEXT NOT NULL,
        received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        source_ip   TEXT NOT NULL DEFAULT ''
    );

    CREATE INDEX IF NOT EXISTS idx_logs_timestamp ON logs(timestamp);
    CREATE INDEX IF NOT EXISTS idx_logs_hostname  ON logs(hostname);
    CREATE INDEX IF NOT EXISTS idx_logs_severity  ON logs(severity);
    CREATE INDEX IF NOT EXISTS idx_logs_app_name  ON logs(app_name);
    CREATE INDEX IF NOT EXISTS idx_logs_host_time ON logs(hostname, timestamp);
    CREATE INDEX IF NOT EXISTS idx_logs_sev_time ON logs(severity, timestamp);
    CREATE INDEX IF NOT EXISTS idx_logs_received_at ON logs(received_at);

    CREATE VIRTUAL TABLE IF NOT EXISTS logs_fts USING fts5(
        message,
        content='logs',
        content_rowid='id',
        tokenize='porter unicode61'
    );

    CREATE TRIGGER IF NOT EXISTS logs_ai AFTER INSERT ON logs BEGIN
        INSERT INTO logs_fts(rowid, message) VALUES (new.id, new.message);
    END;

    CREATE TABLE IF NOT EXISTS hosts (
        hostname    TEXT PRIMARY KEY,
        first_seen  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        last_seen   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        log_count   INTEGER NOT NULL DEFAULT 0
    );
";

/// full-review TH2: every migration was previously tested only from CLEAN
/// temp DBs, so a migration that works against `CREATE`-fresh state but
/// breaks against populated old-shape tables would pass CI and brick real
/// upgrades. This walks the ENTIRE chain against a populated v0.2.6 database
/// and asserts: head version reached, pre-existing rows survive and remain
/// FTS-searchable, and a second run is a no-op.
#[test]
fn full_migration_chain_upgrades_populated_v0_2_6_database() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("v0_2_6-upgrade.db");

    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(V0_2_6_SCHEMA).unwrap();
        for (ts, host, msg) in [
            (
                "2025-06-01T00:00:00Z",
                "old-host-a",
                "legacy kernel panic message",
            ),
            (
                "2025-06-02T00:00:00Z",
                "old-host-b",
                "legacy nginx upstream error",
            ),
        ] {
            conn.execute(
                "INSERT INTO logs (timestamp, hostname, severity, message, raw, received_at, source_ip)
                 VALUES (?1, ?2, 'err', ?3, ?3, ?1, '192.168.1.50:514')",
                rusqlite::params![ts, host, msg],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO hosts (hostname, first_seen, last_seen, log_count)
                 VALUES (?1, ?2, ?2, 1)
                 ON CONFLICT(hostname) DO NOTHING",
                rusqlite::params![host, ts],
            )
            .unwrap();
        }
    }

    // Walk the full migration chain (plus the auto_vacuum conversion VACUUM).
    let config = test_storage_config(db_path.clone());
    let pool = init_pool(&config).expect("full migration chain must apply to a populated old DB");

    let head_version: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert!(
        head_version >= 31,
        "expected migration head >= 31, got {head_version}"
    );

    // Pre-existing rows survived and the FTS index still finds them.
    {
        let conn = pool.get().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "old rows must survive the migration chain");
    }
    let results = crate::db::search_logs(
        &pool,
        &crate::db::SearchParams {
            query: Some("legacy".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(results.len(), 2, "migrated rows must stay FTS-searchable");

    // New-schema columns are live: a current-shape insert works.
    insert_logs_batch(
        &pool,
        &[LogBatchEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            hostname: "new-host".to_string(),
            facility: None,
            severity: "info".to_string(),
            app_name: Some("upgrade-test".to_string()),
            process_id: None,
            message: "post-upgrade insert".to_string(),
            raw: "post-upgrade insert".to_string(),
            source_ip: "127.0.0.1:514".to_string(),
            docker_checkpoint: None,
            ai_tool: Some("claude-code".to_string()),
            ai_project: Some("/tmp/project".to_string()),
            ai_session_id: None,
            ai_transcript_path: None,
            metadata_json: None,
            http_status: None,
            auth_outcome: None,
            dns_blocked: None,
            event_action: None,
            parse_error: None,
        }],
    )
    .expect("current-shape insert must work after upgrade");

    drop(pool);

    // Idempotency: a second init on the upgraded DB is a clean no-op.
    let pool2 = init_pool(&config).expect("re-running init on an upgraded DB must succeed");
    let conn = pool2.get().unwrap();
    let head_again: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(head_again, head_version);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM logs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn migration_37_creates_llm_invocations_table() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = test_storage_config(db_path);
    let pool = init_pool(&config).expect("init_pool should succeed");
    let conn = pool.get().unwrap();

    // Table exists with the exact locked column set.
    let mut stmt = conn
        .prepare(
            "SELECT COUNT(*) FROM pragma_table_info('llm_invocations') WHERE name IN (
                'id','started_at','finished_at','duration_ms','caller_surface','action',
                'provider','model','program','incident_id','ai_tool','ai_project',
                'ai_session_id','evidence_counts_json','prompt_bytes','output_bytes',
                'status','error','metadata_json'
            )",
        )
        .unwrap();
    let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
    assert_eq!(count, 19, "llm_invocations must have all 19 locked columns");
    drop(stmt);

    // Migration is recorded and idempotent.
    let version: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 37",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 1);

    // Re-running init_pool (simulating a restart) must not error or duplicate the row.
    drop(conn);
    drop(pool);
    let pool2 = init_pool(&config).expect("second init_pool should succeed");
    let conn2 = pool2.get().unwrap();
    let version2: i64 = conn2
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 37",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        version2, 1,
        "migration 37 must be idempotent across restarts"
    );
}

#[test]
fn migration_37_indexes_exist() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = test_storage_config(db_path);
    let pool = init_pool(&config).expect("init_pool should succeed");
    let conn = pool.get().unwrap();
    for idx in [
        "idx_llm_invocations_started",
        "idx_llm_invocations_action_started",
        "idx_llm_invocations_status_started",
    ] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = ?1",
                [idx],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "expected index {idx} to exist");
    }
}

// PR #106 reconciliation fix (code-reviewer): if the process is killed
// between `LlmRunner::write_start_row` (status='running') and the matching
// finish-row write, the audit row is orphaned in 'running' forever — no
// process is left to finish it. Authoritative server startup reconciles
// orphaned rows after opening the pool. Query-only CLI processes deliberately
// skip this step because they can coexist with live server-owned work.
#[test]
fn server_start_reconciles_orphaned_running_work_without_pool_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = test_storage_config(db_path);
    let pool = init_pool(&config).expect("init_pool should succeed");
    let conn = pool.get().unwrap();

    conn.execute(
        "INSERT INTO llm_invocations
            (id, started_at, caller_surface, action, provider, status)
         VALUES ('llm-orphan', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'test', 'ai_assess', 'gemini-cli', 'running')",
        [],
    )
    .expect("seed orphaned running row");

    // A concurrently-'success' row (as if it finished cleanly before the
    // crash) must be left untouched by the reconciliation.
    conn.execute(
        "INSERT INTO llm_invocations
            (id, started_at, finished_at, caller_surface, action, provider, status)
         VALUES ('llm-clean', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'test', 'ai_assess', 'gemini-cli', 'success')",
        [],
    )
    .expect("seed clean success row");

    conn.execute(
        "INSERT INTO maintenance_jobs (kind, status, started_at)
         VALUES ('db_integrity', 'running', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        [],
    )
    .expect("seed running maintenance job");

    drop(conn);
    drop(pool);

    // Merely opening another pool, as a local CLI does, must not alter work
    // still owned by the running server.
    let pool2 = init_pool(&config).expect("second init_pool should succeed");
    let conn2 = pool2.get().unwrap();

    let untouched: String = conn2
        .query_row(
            "SELECT status FROM llm_invocations WHERE id = 'llm-orphan'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(untouched, "running");
    let maintenance_untouched: String = conn2
        .query_row(
            "SELECT status FROM maintenance_jobs WHERE kind = 'db_integrity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(maintenance_untouched, "running");
    drop(conn2);

    reconcile_interrupted_server_work(&pool2).unwrap();
    let conn2 = pool2.get().unwrap();

    let (status, finished_at, error): (String, Option<String>, Option<String>) = conn2
        .query_row(
            "SELECT status, finished_at, error FROM llm_invocations WHERE id = 'llm-orphan'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "interrupted");
    assert!(
        finished_at.is_some(),
        "reconciled row must get a finished_at timestamp"
    );
    assert_eq!(error.as_deref(), Some("interrupted by server restart"));

    let (maintenance_status, maintenance_error): (String, String) = conn2
        .query_row(
            "SELECT status, json_extract(result_json, '$.error')
             FROM maintenance_jobs WHERE kind = 'db_integrity'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(maintenance_status, "failed");
    assert_eq!(maintenance_error, "interrupted by server restart");

    let clean_status: String = conn2
        .query_row(
            "SELECT status FROM llm_invocations WHERE id = 'llm-clean'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        clean_status, "success",
        "reconciliation must not touch rows that already reached a terminal status"
    );
}

#[test]
fn migration_38_creates_ai_skill_events_table() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let config = test_storage_config(db_path);
    let pool = init_pool(&config).expect("init_pool should succeed");
    let conn = pool.get().unwrap();

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'ai_skill_events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_exists, 1);

    let indexes: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'ai_skill_events' ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert!(indexes.contains(&"idx_ai_skill_events_timestamp".to_string()));
    assert!(indexes.contains(&"idx_ai_skill_events_skill_time".to_string()));
    assert!(indexes.contains(&"idx_ai_skill_events_plugin_time".to_string()));
    assert!(indexes.contains(&"idx_ai_skill_events_hostname_time".to_string()));
    assert!(indexes.contains(&"idx_ai_skill_events_session_time".to_string()));
    assert!(indexes.contains(&"idx_ai_skill_events_project_skill_time".to_string()));

    // Eng review Fix 5: idx_logs_ai_tool_id lives on the EXISTING `logs`
    // table (backfill keyset-pagination support), not `ai_skill_events`.
    let logs_index_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_logs_ai_tool_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(logs_index_exists, 1);

    // UNIQUE constraint + idempotent re-run of the whole insert on identical
    // (log_id, skill_name, event_kind, evidence_kind) is exercised in Task 6;
    // here we only assert the migration ran and the schema is fully caught up
    // (later migrations, e.g. 39/40, run in the same init_db pass).
    let version = crate::db::read_schema_version_info_conn(&conn)
        .unwrap()
        .version;
    assert_eq!(version, KNOWN_SCHEMA_VERSION);
}

#[test]
fn graph_schema_accepts_entity_resolution_vocabulary() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(
        dir.path().join("resolver-vocab.db"),
    ))
    .unwrap();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES
            ('logical_service', 'plex', 'plex', 'resolver', 'fixture', 'verified'),
            ('service_instance', 'tootie/plex', 'tootie/plex', 'resolver', 'fixture', 'verified')",
        [],
    )
    .unwrap();
    let service = conn
        .query_row(
            "SELECT id FROM graph_entities WHERE entity_type = 'logical_service'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let instance = conn
        .query_row(
            "SELECT id FROM graph_entities WHERE entity_type = 'service_instance'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO graph_relationships
            (relationship_key, src_entity_id, dst_entity_id, relationship_type,
             reason_code, trust_level, confidence)
         VALUES (?1, ?2, ?3, 'instance_of', 'resolver_instance_of', 'verified', 1.0)",
        rusqlite::params![
            format!("{instance}:instance_of:{service}"),
            instance,
            service
        ],
    )
    .unwrap();
}

#[test]
fn migration_41_cleans_legacy_service_rows_from_populated_db() {
    // Simulate a populated pre-41 DB: run all migrations, then re-insert the
    // old-shaped rows a v40 DB could contain and re-run the 41 cleanup SQL by
    // reverting the migration marker before a second init_pool pass.
    //
    // NOTE: this replay runs on a post-41 schema (the CHECK constraints
    // already include the v41 vocabulary), not a byte-faithful v40 schema.
    // The migration's INSERT…SELECT filters are what is under test.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("migration-41-cutover.db");
    {
        let pool = init_pool(&StorageConfig::for_test(db_path.clone())).unwrap();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO graph_entities
                (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
             VALUES
                ('service', 'tootie:plex', 'plex', 'log', 'fixture', 'inferred'),
                ('service', 'tootie:plex:plex', 'tootie/plex/plex', 'log', 'fixture', 'inferred'),
                ('app', 'plex/plex/plex', 'plex/plex/plex', 'log', 'fixture', 'claimed'),
                ('app', 'kernel', 'kernel', 'log', 'fixture', 'claimed')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 41", [])
            .unwrap();
        // Drop the 41-added column so the ALTER TABLE in the replayed
        // migration does not collide.
        conn.execute_batch("ALTER TABLE graph_projection_meta DROP COLUMN projection_contract;")
            .unwrap();
    }
    let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
    let conn = pool.get().unwrap();
    let stale: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_entities
              WHERE entity_type = 'service'
                 OR (entity_type = 'app' AND canonical_key LIKE '%/%/%')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
    // Plain app labels survive the cutover; only nested defect shapes go.
    let plain_app: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'app' AND canonical_key = 'kernel'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(plain_app, 1);
    let contract: String = conn
        .query_row(
            "SELECT projection_contract FROM graph_projection_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        contract,
        crate::db::entity_resolution::vocab::GRAPH_PROJECTION_CONTRACT_V2
    );
}

#[test]
fn migration_41_prunes_relationships_evidence_and_aliases_touching_legacy_entities() {
    // Same replay technique as the cleanup test above (post-41 schema, see
    // its NOTE): seed a legacy `service` entity wired to a surviving host
    // via a relationship with evidence plus an alias, and an unrelated
    // surviving app→host relationship with evidence. Migration 41 must
    // prune everything touching the legacy entity and nothing else, leaving
    // no evidence row pointing at a dead relationship id, and flip a ready
    // projection to stale.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("migration-41-prune.db");
    {
        let pool = init_pool(&StorageConfig::for_test(db_path.clone())).unwrap();
        let conn = pool.get().unwrap();
        let insert_entity = |entity_type: &str, key: &str| -> i64 {
            conn.execute(
                "INSERT INTO graph_entities
                    (entity_type, canonical_key, display_label, source_kind,
                     source_id, trust_level)
                 VALUES (?1, ?2, ?2, 'log', 'fixture', 'inferred')",
                rusqlite::params![entity_type, key],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let insert_rel = |key: &str, src: i64, dst: i64| -> i64 {
            conn.execute(
                "INSERT INTO graph_relationships
                    (relationship_key, src_entity_id, dst_entity_id,
                     relationship_type, reason_code, trust_level, confidence,
                     evidence_count)
                 VALUES (?1, ?2, ?3, 'runs_on', 'log_app_name', 'inferred',
                         0.5, 1)",
                rusqlite::params![key, src, dst],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let insert_evidence = |rel_id: i64, evidence_key: &str| {
            conn.execute(
                "INSERT INTO graph_relationship_evidence
                    (relationship_id, evidence_key, source_kind, source_id,
                     observed_at, reason_code, trust_level, evidence_count)
                 VALUES (?1, ?2, 'log', 'fixture', '2026-01-01T00:00:00Z',
                         'log_app_name', 'inferred', 1)",
                rusqlite::params![rel_id, evidence_key],
            )
            .unwrap();
        };

        let legacy = insert_entity("service", "tootie:plex");
        let host = insert_entity("host", "tootie");
        let app = insert_entity("app", "kernel");
        let legacy_rel = insert_rel("legacy:runs_on:host", legacy, host);
        insert_evidence(legacy_rel, "legacy-evidence");
        conn.execute(
            "INSERT INTO graph_entity_aliases
                (entity_id, alias_type, alias_key, alias_value, trust_level)
             VALUES (?1, 'service_name', 'plex-legacy', 'plex-legacy',
                     'inferred')",
            [legacy],
        )
        .unwrap();
        let surviving_rel = insert_rel("app:runs_on:host", app, host);
        insert_evidence(surviving_rel, "surviving-evidence");

        conn.execute(
            "UPDATE graph_projection_meta SET projection_status = 'ready' WHERE id = 1",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM schema_migrations WHERE version = 41", [])
            .unwrap();
        conn.execute_batch("ALTER TABLE graph_projection_meta DROP COLUMN projection_contract;")
            .unwrap();
    }
    let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
    let conn = pool.get().unwrap();
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |row| row.get(0)).unwrap() };

    // Legacy entity and everything touching it are gone.
    assert_eq!(
        count("SELECT COUNT(*) FROM graph_entities WHERE entity_type = 'service'"),
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM graph_relationships
              WHERE relationship_key = 'legacy:runs_on:host'"
        ),
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM graph_relationship_evidence
              WHERE evidence_key = 'legacy-evidence'"
        ),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM graph_entity_aliases WHERE alias_key = 'plex-legacy'"),
        0
    );

    // The unrelated app→host relationship and its evidence survive.
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM graph_relationships
              WHERE relationship_key = 'app:runs_on:host'"
        ),
        1
    );
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM graph_relationship_evidence
              WHERE evidence_key = 'surviving-evidence'"
        ),
        1
    );

    // Referential integrity: no evidence row references a dead relationship.
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM graph_relationship_evidence e
              WHERE e.relationship_id NOT IN (SELECT id FROM graph_relationships)"
        ),
        0
    );

    // A previously-ready projection is flipped to stale by the migration.
    let status: String = conn
        .query_row(
            "SELECT projection_status FROM graph_projection_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "stale");
}

#[test]
fn migration_42_allows_refuted_alias_trust_level() {
    // Migrations 35/41 added 'refuted' to graph_entities, graph_relationships,
    // and graph_relationship_evidence but missed graph_entity_aliases.
    // Migration 42 widens that CHECK too; assert a fresh DB accepts an alias
    // write at 'refuted' trust without violating the constraint.
    let dir = tempfile::tempdir().unwrap();
    let config = test_storage_config(dir.path().join("migration-42-refuted-alias.db"));
    let pool = init_pool(&config).unwrap();
    let conn = pool.get().unwrap();

    conn.execute(
        "INSERT INTO graph_entities
            (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
         VALUES ('host', 'refuted-alias-host', 'refuted-alias-host', 'log', 'fixture', 'verified')",
        [],
    )
    .unwrap();
    let entity_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO graph_entity_aliases
            (entity_id, alias_type, alias_key, alias_value, source_kind, trust_level)
         VALUES (?1, 'hostname', 'refuted-alias-host', 'refuted-alias-host', 'log', 'refuted')",
        rusqlite::params![entity_id],
    )
    .unwrap();

    let stored_trust: String = conn
        .query_row(
            "SELECT trust_level FROM graph_entity_aliases WHERE entity_id = ?1",
            rusqlite::params![entity_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_trust, "refuted");
}

#[test]
fn migration_42_widens_old_aliases_constraint_and_preserves_rows() {
    // Simulate a populated pre-42 DB: run all migrations, seed an alias row
    // at a pre-refuted trust level, revert the migration 42 marker, then
    // re-run init_pool. The rebuilt table must preserve the existing row and
    // accept a subsequent 'refuted' write.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("migration-42-widen.db");
    let entity_id;
    {
        let pool = init_pool(&StorageConfig::for_test(db_path.clone())).unwrap();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO graph_entities
                (entity_type, canonical_key, display_label, source_kind, source_id, trust_level)
             VALUES ('host', 'pre42-host', 'pre42-host', 'log', 'fixture', 'verified')",
            [],
        )
        .unwrap();
        entity_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO graph_entity_aliases
                (entity_id, alias_type, alias_key, alias_value, source_kind, trust_level)
             VALUES (?1, 'hostname', 'pre42-host', 'pre42-host', 'log', 'claimed')",
            rusqlite::params![entity_id],
        )
        .unwrap();
        conn.execute("DELETE FROM schema_migrations WHERE version = 42", [])
            .unwrap();
    }

    let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
    let conn = pool.get().unwrap();

    let preserved: String = conn
        .query_row(
            "SELECT trust_level FROM graph_entity_aliases WHERE entity_id = ?1",
            rusqlite::params![entity_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preserved, "claimed");

    conn.execute(
        "INSERT INTO graph_entity_aliases
            (entity_id, alias_type, alias_key, alias_value, source_kind, trust_level)
         VALUES (?1, 'service_name', 'pre42-host-refuted', 'pre42-host-refuted', 'log', 'refuted')",
        rusqlite::params![entity_id],
    )
    .unwrap();
}
