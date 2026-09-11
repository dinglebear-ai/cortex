//! Test-only seeding helpers shared by the assessment and reflect service
//! tests. They mirror the private helpers in `src/db/*_incidents_tests.rs`
//! but use timestamps relative to now, so default time windows include them.

use crate::db::{DbPool, LogBatchEntry, insert_logs_batch};

pub(crate) const HOST: &str = "devhost";
pub(crate) const PROJECT: &str = "/tmp/reflect-project";

pub(crate) fn ts_minutes_ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn ai_entry(ts: &str, tool: &str, session_id: &str, message: &str) -> LogBatchEntry {
    LogBatchEntry {
        timestamp: ts.to_string(),
        hostname: HOST.to_string(),
        facility: Some("local0".to_string()),
        severity: "info".to_string(),
        app_name: Some("ai-transcript".to_string()),
        process_id: None,
        message: message.to_string(),
        raw: message.to_string(),
        source_ip: "127.0.0.1:514".to_string(),
        docker_checkpoint: None,
        ai_tool: Some(tool.to_string()),
        ai_project: Some(PROJECT.to_string()),
        ai_session_id: Some(session_id.to_string()),
        ai_transcript_path: Some(format!("{PROJECT}/{session_id}.jsonl")),
        metadata_json: None,
        http_status: None,
        auth_outcome: None,
        dns_blocked: None,
        event_action: None,
        parse_error: None,
    }
}

/// Inserts one log row and returns its id.
fn insert_one(pool: &DbPool, entry: LogBatchEntry) -> i64 {
    insert_logs_batch(pool, std::slice::from_ref(&entry)).unwrap();
    pool.get()
        .unwrap()
        .query_row("SELECT MAX(id) FROM logs", [], |row| row.get(0))
        .unwrap()
}

/// A skill load followed two minutes later by a user correction.
/// `minutes_ago` must be at least 3.
pub(crate) fn seed_skill_incident(pool: &DbPool, session_id: &str, skill: &str, minutes_ago: i64) {
    let skill_ts = ts_minutes_ago(minutes_ago);
    let log_id = insert_one(
        pool,
        ai_entry(
            &skill_ts,
            "codex",
            session_id,
            &format!("loaded skill {skill}"),
        ),
    );
    insert_one(
        pool,
        ai_entry(
            &ts_minutes_ago(minutes_ago - 2),
            "codex",
            session_id,
            "That's not what I asked for, please redo it.",
        ),
    );
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO ai_skill_events
                (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                 skill_name, skill_plugin, event_kind, evidence_kind, created_at)
             VALUES (?1, 'codex', ?2, ?3, ?4, ?5, ?6, NULL, 'skill_invoked', 'transcript', ?5)",
            rusqlite::params![log_id, PROJECT, session_id, HOST, skill_ts, skill],
        )
        .unwrap();
}

/// Two failing calls to the same MCP tool. `minutes_ago` must be at least 2.
pub(crate) fn seed_mcp_incident(
    pool: &DbPool,
    session_id: &str,
    server: &str,
    tool: &str,
    minutes_ago: i64,
) {
    for (offset, call_id) in [(0, "call-1"), (1, "call-2")] {
        let ts = ts_minutes_ago(minutes_ago - offset);
        let log_id = insert_one(
            pool,
            ai_entry(&ts, "claude", session_id, "Error: connection refused"),
        );
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO ai_mcp_events
                    (call_log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                     call_id, tool_name, mcp_server, mcp_tool, event_kind, is_error, created_at)
                 VALUES (?1, 'claude', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'call', 1, ?5)",
                rusqlite::params![
                    log_id,
                    PROJECT,
                    session_id,
                    HOST,
                    ts,
                    call_id,
                    format!("mcp__{server}__{tool}"),
                    server,
                    tool,
                ],
            )
            .unwrap();
    }
}

/// Two failed runs of the same hook. `minutes_ago` must be at least 1.
pub(crate) fn seed_hook_incident(
    pool: &DbPool,
    session_id: &str,
    hook_name: &str,
    minutes_ago: i64,
) {
    for offset in [0, 1] {
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO ai_hook_events
                    (log_id, ai_tool, ai_project, ai_session_id, hostname, timestamp,
                     hook_event, hook_name, status, duration_ms, evidence_kind)
                 VALUES (NULL, 'claude', ?1, ?2, ?3, ?4, 'PostToolUse', ?5, 'failed', NULL,
                         'runtime_transcript')",
                rusqlite::params![
                    PROJECT,
                    session_id,
                    HOST,
                    ts_minutes_ago(minutes_ago - offset),
                    hook_name
                ],
            )
            .unwrap();
    }
}
