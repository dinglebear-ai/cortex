use super::*;
use crate::config::StorageConfig;
use crate::db::init_pool;

#[test]
fn repository_pages_are_stable_when_a_newer_row_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("reads.db"))).unwrap();
    let conn = pool.get().unwrap();
    for (key, seen) in [
        ("a", "2026-08-21T10:00:00Z"),
        ("b", "2026-08-21T11:00:00Z"),
        ("c", "2026-08-21T12:00:00Z"),
    ] {
        conn.execute("INSERT INTO repositories(repository_key,hostname,common_git_dir,primary_path,display_name,first_seen_at,last_seen_at) VALUES(?1,'host','/'||?1||'/git','/'||?1,?1,?2,?2)", rusqlite::params![key,seen]).unwrap();
    }
    drop(conn);
    let query = RepositoryQuery::default();
    let first = list_observatory_repositories(&pool, &query, None, 2).unwrap();
    assert_eq!(
        first.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        vec!["c", "b", "a"]
    );
    let boundary = &first[1];
    pool.get().unwrap().execute("INSERT INTO repositories(repository_key,hostname,common_git_dir,primary_path,display_name,first_seen_at,last_seen_at) VALUES('new','host','/newgit','/new','new','2026-08-21T13:00:00Z','2026-08-21T13:00:00Z')",[]).unwrap();
    let second = list_observatory_repositories(
        &pool,
        &query,
        Some((&boundary.last_seen_at, boundary.id)),
        2,
    )
    .unwrap();
    assert_eq!(
        second.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        vec!["a"]
    );
}

#[test]
fn contract_indexes_cover_run_event_span_and_metric_ordering() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("plans.db"))).unwrap();
    let conn = pool.get().unwrap();
    for (sql, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT id FROM agent_runs WHERE status='active' ORDER BY last_activity_at DESC,id DESC LIMIT 10",
            "idx_agent_runs_status_activity",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM agent_run_events WHERE run_id=1 ORDER BY observed_at DESC,id DESC LIMIT 10",
            "idx_agent_run_events_run_order",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM otel_spans WHERE run_id=1 ORDER BY start_time_unix_nano DESC,id DESC LIMIT 10",
            "idx_otel_spans_run_time",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM otel_metric_points WHERE run_id=1 ORDER BY time_unix_nano DESC,id DESC LIMIT 10",
            "idx_otel_metric_points_run_time",
        ),
    ] {
        let details = conn
            .prepare(sql)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            details.iter().any(|d| d.contains(index)),
            "{index}: {details:?}"
        );
    }
}
