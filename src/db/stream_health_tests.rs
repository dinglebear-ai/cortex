use rusqlite::Connection;

use super::{
    prune_stream_last_seen, refresh_stream_last_seen, silent_streams, stream_last_seen_is_empty,
};

fn conn_with_schema() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "CREATE TABLE stream_last_seen (
             hostname     TEXT NOT NULL,
             source_kind  TEXT NOT NULL,
             last_seen_at TEXT NOT NULL,
             PRIMARY KEY (hostname, source_kind)
         ) WITHOUT ROWID;
         CREATE TABLE logs (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             hostname TEXT NOT NULL,
             source_ip TEXT NOT NULL DEFAULT '',
             ai_transcript_path TEXT,
             metadata_json TEXT,
             received_at TEXT NOT NULL
         );",
    )
    .expect("schema");
    conn
}

/// Insert a log row `age_secs` in the past.
fn insert_log(
    conn: &Connection,
    hostname: &str,
    source_ip: &str,
    metadata: Option<&str>,
    age_secs: i64,
) {
    conn.execute(
        "INSERT INTO logs (hostname, source_ip, metadata_json, received_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', printf('-%d seconds', ?4)))",
        rusqlite::params![hostname, source_ip, metadata, age_secs],
    )
    .expect("insert log");
}

fn rollup_entry(conn: &Connection, hostname: &str, kind: &str) -> Option<String> {
    conn.query_row(
        "SELECT last_seen_at FROM stream_last_seen WHERE hostname = ?1 AND source_kind = ?2",
        rusqlite::params![hostname, kind],
        |row| row.get(0),
    )
    .ok()
}

fn seed_rollup(conn: &Connection, hostname: &str, kind: &str, age_secs: i64) {
    conn.execute(
        "INSERT INTO stream_last_seen (hostname, source_kind, last_seen_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', printf('-%d seconds', ?3)))",
        rusqlite::params![hostname, kind, age_secs],
    )
    .expect("seed rollup");
}

#[test]
fn refresh_classifies_prefixes_and_metadata_kinds() {
    let conn = conn_with_schema();
    insert_log(&conn, "nashost", "docker://nashost/plex/stdout", None, 10);
    insert_log(
        &conn,
        "devhost",
        "192.0.2.6:1234",
        Some(r#"{"source_kind":"agent-docker"}"#),
        10,
    );
    insert_log(&conn, "edgehost", "192.0.2.8:99", None, 10); // no kind — skipped

    refresh_stream_last_seen(&conn, 3600).expect("refresh");

    assert!(rollup_entry(&conn, "nashost", "docker-stream").is_some());
    assert!(rollup_entry(&conn, "devhost", "agent-docker").is_some());
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM stream_last_seen", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "kindless row must not create an entry");
}

#[test]
fn refresh_is_monotonic_and_window_bounded() {
    let conn = conn_with_schema();
    seed_rollup(&conn, "nashost", "syslog-tcp", 30);
    // An older row inside the window must not regress the newer entry.
    insert_log(
        &conn,
        "nashost",
        "1.2.3.4:1",
        Some(r#"{"source_kind":"syslog-tcp"}"#),
        600,
    );
    // A row outside the window must be invisible to the refresh.
    insert_log(
        &conn,
        "backuphost",
        "1.2.3.5:1",
        Some(r#"{"source_kind":"syslog-tcp"}"#),
        7200,
    );

    refresh_stream_last_seen(&conn, 3600).expect("refresh");

    let kept = rollup_entry(&conn, "nashost", "syslog-tcp").expect("entry");
    let age: i64 = conn
        .query_row(
            "SELECT CAST(strftime('%s','now') AS INTEGER) - CAST(strftime('%s', ?1) AS INTEGER)",
            [&kept],
            |r| r.get(0),
        )
        .unwrap();
    assert!(age < 120, "newer rollup value must survive, got age {age}s");
    assert!(
        rollup_entry(&conn, "backuphost", "syslog-tcp").is_none(),
        "row outside window must not enter the rollup"
    );
}

#[test]
fn silent_streams_applies_threshold_forget_and_kind_bounds() {
    let conn = conn_with_schema();
    seed_rollup(&conn, "nashost", "agent-docker", 7200); // silent 2h — alertable
    seed_rollup(&conn, "devhost", "agent-docker", 60); // fresh — not silent
    seed_rollup(&conn, "backuphost", "agent-docker", 700_000); // past forget — ignored
    seed_rollup(&conn, "nashost", "shell-history", 7200); // silent but kind not listed

    let kinds = vec!["agent-docker".to_string()];
    let silent = silent_streams(&conn, &kinds, 3600, 604_800).expect("query");

    assert_eq!(silent.len(), 1, "exactly one alertable stream: {silent:?}");
    assert_eq!(silent[0].hostname, "nashost");
    assert_eq!(silent[0].source_kind, "agent-docker");
    assert!(silent[0].age_secs > 3600 && silent[0].age_secs < 8000);
}

#[test]
fn silent_streams_empty_kinds_returns_nothing() {
    let conn = conn_with_schema();
    seed_rollup(&conn, "nashost", "agent-docker", 7200);
    let silent = silent_streams(&conn, &[], 3600, 604_800).expect("query");
    assert!(silent.is_empty());
}

#[test]
fn prune_drops_only_forgotten_entries() {
    let conn = conn_with_schema();
    seed_rollup(&conn, "nashost", "agent-docker", 700_000);
    seed_rollup(&conn, "devhost", "agent-docker", 60);

    let deleted = prune_stream_last_seen(&conn, 604_800).expect("prune");
    assert_eq!(deleted, 1);
    assert!(rollup_entry(&conn, "devhost", "agent-docker").is_some());
    assert!(rollup_entry(&conn, "nashost", "agent-docker").is_none());
}

#[test]
fn is_empty_reflects_rollup_population() {
    let conn = conn_with_schema();
    assert!(stream_last_seen_is_empty(&conn).unwrap());
    seed_rollup(&conn, "nashost", "agent-docker", 60);
    assert!(!stream_last_seen_is_empty(&conn).unwrap());
}
