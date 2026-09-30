use super::*;

#[derive(Debug, Clone)]
pub struct SessionGraphScope {
    pub project: String,
    pub tool: String,
    pub host: String,
    pub severity_in: Vec<String>,
}

/// Correlate an already resolved session without treating its native ID as a
/// globally unique identity. Graph keys omit the host, so shared keys fall back
/// to the selected session's own evidence instead of attributing other hosts.
pub fn correlate_session_graph_scoped(
    pool: &DbPool,
    session_id: &str,
    scope: &SessionGraphScope,
    limit: usize,
) -> Result<SessionGraphInputs> {
    let limit = limit.clamp(1, 1000);
    let conn = pool.get()?;
    let bounds = conn.query_row(
        "SELECT MIN(timestamp), MAX(timestamp) FROM logs
         WHERE ai_session_id=?1 AND ai_project=?2 AND lower(ai_tool)=lower(?3)
           AND hostname=?4",
        params![session_id, scope.project, scope.tool, scope.host],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    let (Some(start), Some(end)) = bounds else {
        return Ok(SessionGraphInputs::default());
    };
    let key = format!(
        "{}:{}:{}",
        scope.project.trim().to_ascii_lowercase(),
        scope.tool.trim().to_ascii_lowercase(),
        session_id
    );
    let shared_key: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM logs WHERE ai_session_id=?1
         AND lower(trim(ai_project))=lower(trim(?2)) AND lower(trim(ai_tool))=lower(trim(?3))
         AND hostname<>?4)",
        params![session_id, scope.project, scope.tool, scope.host],
        |row| row.get(0),
    )?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM graph_entities WHERE entity_type='ai_session' AND canonical_key=?1)",
        [&key], |row| row.get(0),
    )?;
    let used_graph = exists && !shared_key;
    let mut discovered_hosts = Vec::new();
    let mut discovered_entities = Vec::new();
    if used_graph {
        for entity in super::super::graph::graph_walk_n_hops(&conn, std::slice::from_ref(&key), 2)?
        {
            match entity.entity_type.as_str() {
                super::super::graph::ENTITY_TYPE_HOST => {
                    discovered_hosts.push(entity.canonical_key.clone())
                }
                super::super::graph::ENTITY_TYPE_CONTAINER => {
                    if let Some(host) =
                        super::super::entity_resolution::container_key_host(&entity.canonical_key)
                    {
                        discovered_hosts.push(host.to_string());
                    }
                }
                super::super::graph::ENTITY_TYPE_SERVICE_INSTANCE => {
                    if let Some((host, _)) =
                        super::super::entity_resolution::split_service_instance_key(
                            &entity.canonical_key,
                        )
                    {
                        discovered_hosts.push(host.to_string());
                    }
                }
                _ => {}
            }
            discovered_entities.push(entity.canonical_key);
        }
        discovered_hosts.sort();
        discovered_hosts.dedup();
        discovered_entities.sort();
        discovered_entities.dedup();
    }
    let mut bindings: Vec<rusqlite::types::Value> = vec![
        session_id.to_string().into(),
        scope.project.clone().into(),
        scope.tool.clone().into(),
        scope.host.clone().into(),
        start.clone().into(),
        end.clone().into(),
    ];
    let own =
        "(l.ai_session_id=?1 AND l.ai_project=?2 AND lower(l.ai_tool)=lower(?3) AND l.hostname=?4)";
    let mut selection = own.to_string();
    if used_graph && !discovered_hosts.is_empty() {
        let first_host = bindings.len() + 1;
        bindings.extend(
            discovered_hosts
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text),
        );
        let hosts = (first_host..=bindings.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        // Host evidence is a temporal correlation, never another AI session's
        // transcript presented as part of the selected identity.
        selection = format!("({own} OR (l.ai_session_id IS NULL AND l.hostname IN ({hosts})))");
    }
    let severity = if scope.severity_in.is_empty() {
        String::new()
    } else {
        let first = bindings.len() + 1;
        bindings.extend(
            scope
                .severity_in
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text),
        );
        let placeholders = (first..=bindings.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND l.severity IN ({placeholders})")
    };
    bindings.push((limit as i64).into());
    // Cap text before materializing rows. JSON metadata is omitted intact
    // rather than returning an invalid JSON prefix; propagate the omission.
    let columns = FTS_SELECT_COLS
        .replace("l.message", "substr(l.message,1,2048)")
        .replace(
            "l.metadata_json",
            "CASE WHEN length(l.metadata_json)>4096 THEN NULL ELSE l.metadata_json END",
        );
    let sql = format!(
        "SELECT {columns},(length(l.message)>2048 OR COALESCE(length(l.metadata_json)>4096,0)) FROM logs l WHERE {selection}
        AND l.timestamp>=?5 AND l.timestamp<=?6{severity} ORDER BY l.timestamp DESC,l.id DESC LIMIT ?{}",
        bindings.len()
    );
    let rows = conn
        .prepare(&sql)?
        .query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            Ok((map_row(row)?, row.get::<_, bool>(15)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let source_fields_truncated = rows.iter().any(|(_, truncated)| *truncated);
    let logs = rows.into_iter().map(|(entry, _)| entry).collect();
    Ok(SessionGraphInputs {
        bounds: Some((start, end)),
        session_entity_keys: if used_graph { vec![key] } else { Vec::new() },
        discovered_hosts,
        discovered_entities,
        used_graph,
        logs,
        source_fields_truncated,
    })
}

#[cfg(test)]
#[path = "queries_session_graph_tests.rs"]
mod tests;
