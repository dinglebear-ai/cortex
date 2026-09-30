//! Safe structured evidence derived from provider-native transcript records.

use super::*;

pub(super) fn attach_structured_events(
    record: &mut AiTranscriptRecord,
    source_kind: scanner::SourceKind,
    raw_line: &str,
) {
    if !matches!(
        source_kind,
        scanner::SourceKind::ClaudeProject | scanner::SourceKind::CodexSession
    ) {
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_line) else {
        return;
    };
    let mcp = match source_kind {
        scanner::SourceKind::ClaudeProject => {
            scanner::mcp_events::extract_claude_mcp_events(&value)
        }
        scanner::SourceKind::CodexSession => scanner::mcp_events::extract_codex_mcp_events(&value),
        _ => unreachable!(),
    };
    if mcp.len() > MAX_STRUCTURED_EVENTS_PER_RECORD {
        record.envelope.diagnostics.push(EvidenceDiagnostic {
            code: "mcp_event_limit".to_string(),
            detail: None,
        });
    }
    record.envelope.mcp_events = mcp
        .into_iter()
        .take(MAX_STRUCTURED_EVENTS_PER_RECORD)
        .map(|event| ForwardedMcpEvent {
            call_id: sha256_id([b"mcp-call-id".as_slice(), event.call_id.as_bytes()]),
            tool_name: crate::assessment::redact_secrets(&truncate_utf8(&event.tool_name, 256)),
            event_kind: event.event_kind.as_str().to_string(),
            status: event
                .status
                .map(|status| crate::assessment::redact_secrets(&truncate_utf8(&status, 128))),
            is_error: event.is_error,
        })
        .collect();
    if source_kind == scanner::SourceKind::ClaudeProject {
        let hooks = scanner::hook_events::extract_claude_hook_events(&value);
        if hooks.len() > MAX_STRUCTURED_EVENTS_PER_RECORD {
            record.envelope.diagnostics.push(EvidenceDiagnostic {
                code: "hook_event_limit".to_string(),
                detail: None,
            });
        }
        record.envelope.hook_events = hooks
            .into_iter()
            .take(MAX_STRUCTURED_EVENTS_PER_RECORD)
            .map(|event| ForwardedHookEvent {
                hook_event: crate::assessment::redact_secrets(&truncate_utf8(
                    &event.hook_event,
                    256,
                )),
                hook_name: event
                    .hook_name
                    .map(|name| crate::assessment::redact_secrets(&truncate_utf8(&name, 256))),
                status: event.status.as_str().to_string(),
                exit_code: event.exit_code,
                duration_ms: event.duration_ms,
            })
            .collect();
    }
}
