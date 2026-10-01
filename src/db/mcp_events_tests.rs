use super::*;
use crate::config::StorageConfig;
use crate::db::pool::init_pool;
use crate::scanner::mcp_events::McpEventKind;

fn test_pool() -> (crate::db::DbPool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = init_pool(&StorageConfig::for_test(db_path)).unwrap();
    (pool, dir)
}

fn insert_log_row(pool: &crate::db::DbPool, hostname: &str, timestamp: &str) -> i64 {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO logs (timestamp, hostname, severity, message, raw, source_ip)
         VALUES (?1, ?2, 'info', 'msg', 'raw', 'transcript://claude_project')",
        rusqlite::params![timestamp, hostname],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn call_event(
    call_id: &str,
    tool_name: &str,
    mcp_server: Option<&str>,
    mcp_tool: Option<&str>,
) -> ExtractedMcpEvent {
    ExtractedMcpEvent {
        call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        mcp_server: mcp_server.map(str::to_string),
        mcp_tool: mcp_tool.map(str::to_string),
        event_kind: McpEventKind::Call,
        turn_id: None,
        status: None,
        is_error: None,
        arguments_json: Some("{}".to_string()),
        output_preview: None,
        error_text: None,
    }
}

fn result_event(call_id: &str, is_error: bool) -> ExtractedMcpEvent {
    ExtractedMcpEvent {
        call_id: call_id.to_string(),
        tool_name: String::new(),
        mcp_server: None,
        mcp_tool: None,
        event_kind: McpEventKind::Result,
        turn_id: None,
        status: Some(if is_error { "error" } else { "ok" }.to_string()),
        is_error: Some(is_error),
        arguments_json: None,
        output_preview: (!is_error).then(|| "ok output".to_string()),
        error_text: is_error.then(|| "boom".to_string()),
    }
}

#[test]
fn insert_and_list_round_trips_a_call_event() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = McpEventInsert {
        log_id,
        ai_tool: "claude".to_string(),
        ai_project: Some("cortex".to_string()),
        ai_session_id: Some("sess-1".to_string()),
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: call_event(
            "toolu_1",
            "mcp__labby__search",
            Some("labby"),
            Some("search"),
        ),
    };
    let inserted = insert_mcp_events(&pool, &[insert]).unwrap();
    assert_eq!(inserted, 1);

    let result = list_mcp_events(&pool, &AiMcpEventParams::default()).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].call_id, "toolu_1");
    assert_eq!(result.events[0].tool_name, "mcp__labby__search");
    assert_eq!(result.events[0].mcp_server.as_deref(), Some("labby"));
    assert_eq!(result.events[0].mcp_tool.as_deref(), Some("search"));
    assert_eq!(result.events[0].event_kind, "call");
    assert_eq!(result.events[0].call_log_id, Some(log_id));
    assert_eq!(result.events[0].result_log_id, None);
}

#[test]
fn insert_or_ignore_is_idempotent_on_duplicate() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = McpEventInsert {
        log_id,
        ai_tool: "claude".to_string(),
        ai_project: None,
        ai_session_id: None,
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: call_event("toolu_dup", "Bash", None, None),
    };
    assert_eq!(
        insert_mcp_events(&pool, std::slice::from_ref(&insert)).unwrap(),
        1
    );
    assert_eq!(insert_mcp_events(&pool, &[insert]).unwrap(), 0);

    let result = list_mcp_events(&pool, &AiMcpEventParams::default()).unwrap();
    assert_eq!(result.total, 1);
}

#[test]
fn result_event_resolves_tool_name_from_paired_call_row() {
    let (pool, _dir) = test_pool();
    let call_log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let result_log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:01.000Z");

    insert_mcp_events(
        &pool,
        &[
            McpEventInsert {
                log_id: call_log_id,
                ai_tool: "claude".to_string(),
                ai_project: Some("cortex".to_string()),
                ai_session_id: Some("sess-1".to_string()),
                hostname: "devhost".to_string(),
                timestamp: "2026-06-01T00:00:00.000Z".to_string(),
                event: call_event(
                    "toolu_paired",
                    "mcp__gh__search",
                    Some("gh"),
                    Some("search"),
                ),
            },
            McpEventInsert {
                log_id: result_log_id,
                ai_tool: "claude".to_string(),
                ai_project: Some("cortex".to_string()),
                ai_session_id: Some("sess-1".to_string()),
                hostname: "devhost".to_string(),
                timestamp: "2026-06-01T00:00:01.000Z".to_string(),
                event: result_event("toolu_paired", false),
            },
        ],
    )
    .unwrap();

    let result = list_mcp_events(
        &pool,
        &AiMcpEventParams {
            mcp_server: Some("gh".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 2);
    let result_row = result
        .events
        .iter()
        .find(|e| e.event_kind == "result")
        .expect("result row present");
    assert_eq!(result_row.tool_name, "mcp__gh__search");
    assert_eq!(result_row.mcp_server.as_deref(), Some("gh"));
    assert_eq!(result_row.mcp_tool.as_deref(), Some("search"));
    assert_eq!(result_row.result_log_id, Some(result_log_id));
    assert_eq!(result_row.is_error, Some(false));
}

#[test]
fn result_event_without_paired_call_still_inserts_unclassified() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let insert = McpEventInsert {
        log_id,
        ai_tool: "claude".to_string(),
        ai_project: None,
        ai_session_id: None,
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event: result_event("toolu_orphan", true),
    };
    let inserted = insert_mcp_events(&pool, &[insert]).unwrap();
    assert_eq!(inserted, 1);
    let result = list_mcp_events(&pool, &AiMcpEventParams::default()).unwrap();
    assert_eq!(result.events[0].tool_name, "");
    assert_eq!(result.events[0].mcp_server, None);
    assert_eq!(result.events[0].is_error, Some(true));
}

#[test]
fn late_call_and_replays_resolve_result_identity_without_replacing_outcome() {
    for session in [None, Some("sess-1")] {
        let (pool, _dir) = test_pool();
        let result_log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:01.000Z");
        let call_log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
        let result = McpEventInsert {
            log_id: result_log_id,
            ai_tool: "claude".to_string(),
            ai_project: None,
            ai_session_id: session.map(str::to_string),
            hostname: "devhost".to_string(),
            timestamp: "2026-06-01T00:00:01.000Z".to_string(),
            event: result_event("late-call", true),
        };
        let call = McpEventInsert {
            log_id: call_log_id,
            ai_tool: "claude".to_string(),
            ai_project: None,
            ai_session_id: session.map(str::to_string),
            hostname: "devhost".to_string(),
            timestamp: "2026-06-01T00:00:00.000Z".to_string(),
            event: call_event("late-call", "mcp__gh__search", Some("gh"), Some("search")),
        };
        assert_eq!(
            insert_mcp_events(&pool, std::slice::from_ref(&result)).unwrap(),
            1
        );
        assert_eq!(
            insert_mcp_events(&pool, std::slice::from_ref(&call)).unwrap(),
            1
        );
        let assert_result = || {
            let rows = list_mcp_events(
                &pool,
                &AiMcpEventParams {
                    mcp_server: Some("gh".to_string()),
                    is_error: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                rows.total, 1,
                "failed result must be visible through MCP filters"
            );
            let row = &rows.events[0];
            assert_eq!(row.tool_name, "mcp__gh__search");
            assert_eq!(row.mcp_tool.as_deref(), Some("search"));
            assert_eq!(row.result_log_id, Some(result_log_id));
            assert_eq!(row.status.as_deref(), Some("error"));
            assert_eq!(row.error_text.as_deref(), Some("boom"));
            assert_eq!(row.timestamp, result.timestamp);
        };
        assert_result();
        // Old orphan rows can also be repaired by replay without a new insert.
        {
            let conn = pool.get().unwrap();
            conn.execute("UPDATE ai_mcp_events SET tool_name = '', mcp_server = NULL, mcp_tool = NULL WHERE event_kind = 'result'", []).unwrap();
        }
        assert_eq!(
            insert_mcp_events(&pool, std::slice::from_ref(&result)).unwrap(),
            0
        );
        assert_result();
        let mut conflicting_call = call;
        conflicting_call.event = call_event(
            "late-call",
            "mcp__other__wrong",
            Some("other"),
            Some("wrong"),
        );
        assert_eq!(insert_mcp_events(&pool, &[conflicting_call]).unwrap(), 0);
        assert_result();
    }
}

#[test]
fn late_call_preserves_explicit_result_identity_and_other_sessions() {
    let (pool, _dir) = test_pool();
    let log_id = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let make = |session: &str, event| McpEventInsert {
        log_id,
        ai_tool: "claude".to_string(),
        ai_project: None,
        ai_session_id: Some(session.to_string()),
        hostname: "devhost".to_string(),
        timestamp: "2026-06-01T00:00:00.000Z".to_string(),
        event,
    };
    let mut named_result = result_event("same-id", true);
    named_result.tool_name = "explicit-tool".to_string();
    insert_mcp_events(
        &pool,
        &[
            make("session-a", named_result),
            make("session-b", result_event("same-id", true)),
            make(
                "session-a",
                call_event("same-id", "mcp__gh__search", Some("gh"), Some("search")),
            ),
        ],
    )
    .unwrap();
    let rows = list_mcp_events(&pool, &AiMcpEventParams::default()).unwrap();
    let results = rows
        .events
        .iter()
        .filter(|event| event.event_kind == "result")
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .any(|event| event.ai_session_id.as_deref() == Some("session-a")
                && event.tool_name == "explicit-tool")
    );
    assert!(
        results
            .iter()
            .any(|event| event.ai_session_id.as_deref() == Some("session-b")
                && event.tool_name.is_empty()
                && event.mcp_server.is_none())
    );
}

#[test]
fn list_filters_by_mcp_server_project_and_is_error() {
    let (pool, _dir) = test_pool();
    let log_id_a = insert_log_row(&pool, "devhost", "2026-06-01T00:00:00.000Z");
    let log_id_b = insert_log_row(&pool, "nashost", "2026-06-01T01:00:00.000Z");
    insert_mcp_events(
        &pool,
        &[
            McpEventInsert {
                log_id: log_id_a,
                ai_tool: "claude".to_string(),
                ai_project: Some("cortex".to_string()),
                ai_session_id: Some("sess-a".to_string()),
                hostname: "devhost".to_string(),
                timestamp: "2026-06-01T00:00:00.000Z".to_string(),
                event: call_event(
                    "toolu_a",
                    "mcp__labby__search",
                    Some("labby"),
                    Some("search"),
                ),
            },
            McpEventInsert {
                log_id: log_id_b,
                ai_tool: "codex".to_string(),
                ai_project: Some("axon".to_string()),
                ai_session_id: Some("sess-b".to_string()),
                hostname: "nashost".to_string(),
                timestamp: "2026-06-01T01:00:00.000Z".to_string(),
                event: {
                    let mut e =
                        call_event("toolu_b", "mcp__gh__search", Some("gh"), Some("search"));
                    e.is_error = Some(true);
                    e
                },
            },
        ],
    )
    .unwrap();

    let result = list_mcp_events(
        &pool,
        &AiMcpEventParams {
            mcp_server: Some("labby".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].call_id, "toolu_a");

    let result = list_mcp_events(
        &pool,
        &AiMcpEventParams {
            is_error: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].call_id, "toolu_b");
}
