//! Transactional persistence for structured transcript tool and hook events.

use super::*;

pub(super) struct ForwardedEventContext {
    pub(super) log_id: i64,
    pub(super) ai_tool: String,
    pub(super) ai_project: Option<String>,
    pub(super) ai_session_id: Option<String>,
    pub(super) hostname: String,
    pub(super) timestamp: String,
}

pub(super) fn insert_forwarded_events_in_tx(
    tx: &rusqlite::Transaction<'_>,
    envelope: &EvidenceEnvelope,
    context: &ForwardedEventContext,
) -> anyhow::Result<()> {
    let mcp_events = envelope
        .mcp_events
        .iter()
        .map(|forwarded| {
            let (mcp_server, mcp_tool) =
                crate::scanner::mcp_events::classify_tool_name(&forwarded.tool_name);
            db::McpEventInsert {
                log_id: context.log_id,
                ai_tool: context.ai_tool.clone(),
                ai_project: context.ai_project.clone(),
                ai_session_id: context.ai_session_id.clone(),
                hostname: context.hostname.clone(),
                timestamp: context.timestamp.clone(),
                event: crate::scanner::mcp_events::ExtractedMcpEvent {
                    call_id: forwarded.call_id.clone(),
                    tool_name: forwarded.tool_name.clone(),
                    mcp_server,
                    mcp_tool,
                    event_kind: if forwarded.event_kind == "call" {
                        crate::scanner::mcp_events::McpEventKind::Call
                    } else {
                        crate::scanner::mcp_events::McpEventKind::Result
                    },
                    turn_id: None,
                    status: forwarded.status.clone(),
                    is_error: forwarded.is_error,
                    arguments_json: None,
                    output_preview: None,
                    error_text: None,
                },
            }
        })
        .collect::<Vec<_>>();
    db::insert_mcp_events_in_tx(tx, &mcp_events)?;
    let hook_events = envelope
        .hook_events
        .iter()
        .map(|forwarded| {
            let status = match forwarded.status.as_str() {
                "success" => crate::scanner::hook_events::HookStatus::Success,
                "failed" => crate::scanner::hook_events::HookStatus::Failed,
                "blocked" => crate::scanner::hook_events::HookStatus::Blocked,
                "error" => crate::scanner::hook_events::HookStatus::Error,
                _ => crate::scanner::hook_events::HookStatus::Unknown,
            };
            db::HookEventInsert {
                log_id: Some(context.log_id),
                ai_tool: context.ai_tool.clone(),
                ai_project: context.ai_project.clone(),
                ai_session_id: context.ai_session_id.clone(),
                hostname: context.hostname.clone(),
                timestamp: context.timestamp.clone(),
                event: crate::scanner::hook_events::ExtractedHookEvent {
                    hook_event: forwarded.hook_event.clone(),
                    hook_name: forwarded.hook_name.clone(),
                    hook_source: None,
                    hook_command: None,
                    status,
                    exit_code: forwarded.exit_code,
                    duration_ms: forwarded.duration_ms,
                    stdout_preview: None,
                    stderr_preview: None,
                    persisted_output_path: None,
                    trusted_hash: None,
                    evidence_kind: crate::scanner::hook_events::HookEvidenceKind::RuntimeTranscript,
                    metadata_json: None,
                },
            }
        })
        .collect::<Vec<_>>();
    db::insert_hook_events_in_tx(tx, &hook_events)?;
    Ok(())
}
