use super::*;
use crate::config::StorageConfig;
use crate::db::init_pool;
use std::sync::Arc;

#[tokio::test]
async fn missing_exact_incident_preserves_incomplete_search() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::for_test(dir.path().join("incident-cutoff.db"));
    let pool = Arc::new(init_pool(&storage).unwrap());
    let service = CortexService::new(Arc::clone(&pool), storage);
    {
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "WITH RECURSIVE samples(n) AS (
                 VALUES(1) UNION ALL SELECT n+1 FROM samples WHERE n<501
             )
             INSERT INTO logs(timestamp,hostname,severity,message,raw,
                              ai_tool,ai_project,ai_session_id)
             SELECT '2026-10-04T00:00:00.000Z','host','info','permission denied',
                    'permission denied','codex','project','session' FROM samples;
             INSERT INTO ai_skill_events(log_id,ai_tool,ai_project,ai_session_id,
                 hostname,timestamp,skill_name,event_kind,evidence_kind)
             VALUES(1,'codex','project','session','host','2026-10-04T00:00:00.000Z',
                    'skill','skill_invoked','transcript');
             INSERT INTO ai_mcp_events(call_log_id,ai_tool,ai_project,ai_session_id,
                 hostname,timestamp,call_id,tool_name,mcp_server,event_kind)
             VALUES(1,'codex','project','session','host','2026-10-04T00:00:00.000Z',
                    'call','mcp__server__tool','server','call');
             INSERT INTO ai_hook_events(log_id,ai_tool,ai_project,ai_session_id,
                 hostname,timestamp,hook_event,status,evidence_kind)
             VALUES(1,'codex','project','session','host','2026-10-04T00:00:00.000Z',
                    'PreToolUse','success','runtime_transcript');",
        )
        .unwrap();
    }

    // The exact ID is absent, but each domain's anchor scan hit its bound.
    // Returning no evidence must preserve that uncertainty at the API boundary.
    let skill = service
        .investigate_ai_skill_incidents(AiSkillInvestigateRequest {
            incident_id: Some("missing-skill-incident".into()),
            since: Some("2026-10-04T00:00:00Z".into()),
            until: Some("2026-10-04T00:10:00Z".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(skill.evidence.is_empty());
    assert!(skill.no_data);
    assert!(skill.truncated);

    let mcp = service
        .investigate_ai_mcp_incidents(AiMcpInvestigateRequest {
            incident_id: Some("missing-mcp-incident".into()),
            since: Some("2026-10-04T00:00:00Z".into()),
            until: Some("2026-10-04T00:10:00Z".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(mcp.evidence.is_empty());
    assert!(mcp.no_data);
    assert!(mcp.truncated);

    let hook = service
        .investigate_ai_hook_incidents(AiHookInvestigateRequest {
            incident_id: Some("missing-hook-incident".into()),
            since: Some("2026-10-04T00:00:00Z".into()),
            until: Some("2026-10-04T00:10:00Z".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(hook.evidence.is_empty());
    assert!(hook.no_data);
    assert!(hook.truncated);
}
