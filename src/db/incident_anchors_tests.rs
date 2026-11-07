use super::*;

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE logs(id INTEGER PRIMARY KEY, message TEXT, metadata_json TEXT,
        ai_session_id TEXT, ai_project TEXT, ai_tool TEXT, hostname TEXT, timestamp TEXT);
        CREATE INDEX anchors ON logs(ai_session_id,ai_project,ai_tool,hostname,timestamp);",
    )
    .unwrap();
    conn
}

fn window(host: &str) -> AnchorWindow {
    AnchorWindow::new(
        "session",
        "project",
        "codex",
        host,
        "2026-10-04T00:00:00.000Z",
        "2026-10-04T00:00:00.000Z",
        600,
    )
}

fn insert(conn: &Connection, id: i64, host: &str) {
    conn.execute(
        "INSERT INTO logs VALUES(?1,'failure','{}','session','project','codex',?2,
        '2026-10-04T00:00:01.000Z')",
        rusqlite::params![id, host],
    )
    .unwrap();
}

#[test]
fn deduplicates_windows_and_keeps_hosts_separate() {
    let conn = database();
    insert(&conn, 1, "host-a");
    insert(&conn, 2, "host-b");
    let cache = load_anchors(
        &conn,
        [window("host-a"), window("host-a"), window("host-b")],
    )
    .unwrap();
    assert_eq!(cache.queries, 1);
    assert_eq!(cache.windows.len(), 2);
    assert_eq!(cache.get(&window("host-a")).rows[0].id, 1);
    assert_eq!(cache.get(&window("host-b")).rows[0].id, 2);
    assert!(!cache.truncated());
}

#[test]
fn overflow_marks_only_the_incomplete_window() {
    let conn = database();
    for id in 1..=501 {
        insert(&conn, id, "host-a");
    }
    insert(&conn, 502, "host-b");
    let cache = load_anchors(&conn, [window("host-a"), window("host-b")]).unwrap();
    assert_eq!(cache.get(&window("host-a")).rows.len(), 500);
    assert!(cache.get(&window("host-a")).truncated);
    assert!(!cache.get(&window("host-b")).truncated);
    assert!(cache.truncated());
}

#[test]
fn request_budget_marks_unvisited_windows_incomplete_deterministically() {
    let conn = database();
    for id in 1..=501 {
        insert(&conn, id, "host-a");
    }
    insert(&conn, 502, "host-b");
    let cache = load_with_budget(&conn, [window("host-b"), window("host-a")], 501).unwrap();
    assert_eq!(cache.queries, 1);
    assert_eq!(cache.get(&window("host-a")).rows.len(), 500);
    assert!(cache.get(&window("host-b")).rows.is_empty());
    assert!(cache.get(&window("host-b")).truncated);
}

#[test]
fn many_distinct_windows_use_batches_not_one_query_per_incident() {
    let conn = database();
    let windows: Vec<_> = (0..200).map(|i| window(&format!("host-{i}"))).collect();
    let cache = load_anchors(&conn, windows).unwrap();
    assert_eq!(cache.queries, 4);
    assert!(!cache.truncated());
}

#[test]
fn real_pool_plan_uses_host_session_time_index_and_primary_key_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let pool = super::super::pool::init_pool(&crate::config::StorageConfig::for_test(
        dir.path().join("db"),
    ))
    .unwrap();
    let conn = pool.get().unwrap();
    let sql = format!("EXPLAIN QUERY PLAN {}", batch_sql(1));
    let mut stmt = conn.prepare(&sql).unwrap();
    let details = stmt
        .query_map(
            rusqlite::params![
                0,
                "session",
                "project",
                "codex",
                "host",
                "2026-10-04T00:00:00Z",
                "2026-10-04T00:10:00Z"
            ],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        details.contains("idx_logs_ai_session_host_time"),
        "{details}"
    );
    assert!(
        details.contains("SEARCH l USING INTEGER PRIMARY KEY"),
        "{details}"
    );
}
