//! Stable, bounded pages over the committed graph projection and its change journal.
//! Every page is read in one SQLite transaction so a concurrent rebuild cannot
//! combine a cursor from one projection with rows from another.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::DbPool;
use super::graph::{self, GraphEntityRow, GraphRelationshipRow};

#[derive(Debug)]
pub struct SnapshotPage<T> {
    pub rows: Vec<T>,
    pub anchor: i64,
    pub has_more: bool,
    pub changed: bool,
    pub projection_status: String,
    pub source_watermark: String,
    pub last_completed_at: Option<String>,
    pub is_degraded: bool,
}

#[derive(Debug)]
pub struct ChangeRow {
    pub seq: i64,
    pub object_kind: String,
    pub operation: String,
    pub item_key: String,
    pub occurred_at: String,
    pub entity: Option<GraphEntityRow>,
    pub relationship: Option<GraphRelationshipRow>,
    pub evidence_ids: Vec<i64>,
    pub source_kinds: Vec<String>,
}

#[derive(Debug)]
pub struct RelationshipItem {
    pub row: GraphRelationshipRow,
    pub evidence_ids: Vec<i64>,
    pub source_kinds: Vec<String>,
}

pub struct EntityFilters<'a> {
    pub entity_type: &'a str,
    pub source_kind: &'a str,
    pub trust_level: &'a str,
    pub query: &'a str,
}

#[derive(Debug)]
pub struct ChangePage {
    pub rows: Vec<ChangeRow>,
    pub latest: i64,
    pub has_more: bool,
    pub expired: bool,
    pub projection_status: String,
    pub source_watermark: String,
    pub last_completed_at: Option<String>,
    pub is_degraded: bool,
}

fn sequence(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'graph_change_events'), 0)",
        [],
        |row| row.get(0),
    )?)
}

fn metadata(conn: &rusqlite::Connection) -> Result<(String, String, Option<String>, bool)> {
    Ok(conn.query_row(
        "SELECT projection_status, source_watermark, last_completed_at, is_degraded
         FROM graph_projection_meta WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3)? != 0,
            ))
        },
    )?)
}

pub fn entities(
    pool: &DbPool,
    after: i64,
    expected_anchor: Option<i64>,
    limit: u32,
    filters: EntityFilters<'_>,
) -> Result<SnapshotPage<GraphEntityRow>> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let anchor = sequence(&tx)?;
    let (projection_status, source_watermark, last_completed_at, is_degraded) = metadata(&tx)?;
    if expected_anchor.is_some_and(|expected| expected != anchor) {
        return Ok(SnapshotPage {
            rows: vec![],
            anchor,
            has_more: false,
            changed: true,
            projection_status,
            source_watermark,
            last_completed_at,
            is_degraded,
        });
    }
    let mut stmt = tx.prepare(
        "SELECT id, entity_type, canonical_key, display_label, source_kind, source_id,
                trust_level, first_seen_at, last_seen_at
         FROM graph_entities
         WHERE id > ?1 AND (?2 = '' OR entity_type = ?2)
           AND (?3 = '' OR source_kind = ?3) AND (?4 = '' OR trust_level = ?4)
           AND (?5 = '' OR instr(lower(display_label), lower(?5)) > 0
                OR instr(lower(canonical_key), lower(?5)) > 0)
         ORDER BY id LIMIT ?6",
    )?;
    let mut rows = stmt
        .query_map(
            params![
                after,
                filters.entity_type,
                filters.source_kind,
                filters.trust_level,
                filters.query,
                i64::from(limit) + 1
            ],
            graph::graph_entity_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    Ok(SnapshotPage {
        rows,
        anchor,
        has_more,
        changed: false,
        projection_status,
        source_watermark,
        last_completed_at,
        is_degraded,
    })
}

pub fn relationships(
    pool: &DbPool,
    after: i64,
    expected_anchor: Option<i64>,
    limit: u32,
) -> Result<SnapshotPage<RelationshipItem>> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let anchor = sequence(&tx)?;
    let (projection_status, source_watermark, last_completed_at, is_degraded) = metadata(&tx)?;
    if expected_anchor.is_some_and(|expected| expected != anchor) {
        return Ok(SnapshotPage {
            rows: vec![],
            anchor,
            has_more: false,
            changed: true,
            projection_status,
            source_watermark,
            last_completed_at,
            is_degraded,
        });
    }
    let mut stmt = tx.prepare(
        "SELECT id, relationship_key, src_entity_id, dst_entity_id, relationship_type,
                reason_code, trust_level, confidence, evidence_count, first_seen_at, last_seen_at
         FROM graph_relationships WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    let mut rows = stmt
        .query_map(
            params![after, i64::from(limit) + 1],
            graph::graph_relationship_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let rows = rows
        .into_iter()
        .map(|row| {
            let (evidence_ids, source_kinds) = relationship_provenance(&tx, row.id)?;
            Ok(RelationshipItem {
                row,
                evidence_ids,
                source_kinds,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SnapshotPage {
        rows,
        anchor,
        has_more,
        changed: false,
        projection_status,
        source_watermark,
        last_completed_at,
        is_degraded,
    })
}

pub fn changes(pool: &DbPool, after: i64, limit: u32) -> Result<ChangePage> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let latest = sequence(&tx)?;
    let oldest: Option<i64> =
        tx.query_row("SELECT MIN(seq) FROM graph_change_events", [], |row| {
            row.get(0)
        })?;
    let (projection_status, source_watermark, last_completed_at, is_degraded) = metadata(&tx)?;
    let expired = after < oldest.unwrap_or(latest + 1) - 1 || after > latest;
    if expired {
        return Ok(ChangePage {
            rows: vec![],
            latest,
            has_more: false,
            expired,
            projection_status,
            source_watermark,
            last_completed_at,
            is_degraded,
        });
    }
    let mut stmt = tx.prepare(
        "SELECT seq, object_kind, operation, item_key, occurred_at
         FROM graph_change_events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
    )?;
    let mut rows = stmt
        .query_map(params![after, i64::from(limit) + 1], |row| {
            Ok(ChangeRow {
                seq: row.get(0)?,
                object_kind: row.get(1)?,
                operation: row.get(2)?,
                item_key: row.get(3)?,
                occurred_at: row.get(4)?,
                entity: None,
                relationship: None,
                evidence_ids: Vec::new(),
                source_kinds: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    for event in &mut rows {
        if event.operation != "upsert" {
            continue;
        }
        if event.object_kind == "entity" {
            if let Some((kind, key)) = event.item_key.split_once('\u{1f}') {
                event.entity = tx.query_row(
                    "SELECT id, entity_type, canonical_key, display_label, source_kind, source_id,
                            trust_level, first_seen_at, last_seen_at
                     FROM graph_entities WHERE entity_type = ?1 AND canonical_key = ?2",
                    params![kind, key], graph::graph_entity_from_row,
                ).optional()?;
            }
        } else {
            event.relationship = tx.query_row(
                "SELECT id, relationship_key, src_entity_id, dst_entity_id, relationship_type,
                        reason_code, trust_level, confidence, evidence_count, first_seen_at, last_seen_at
                 FROM graph_relationships WHERE relationship_key = ?1",
                [&event.item_key], graph::graph_relationship_from_row,
            ).optional()?;
            if let Some(relationship) = &event.relationship {
                (event.evidence_ids, event.source_kinds) =
                    relationship_provenance(&tx, relationship.id)?;
            }
        }
        if event.entity.is_none() && event.relationship.is_none() {
            event.operation = "delete".into();
        }
    }
    Ok(ChangePage {
        rows,
        latest,
        has_more,
        expired: false,
        projection_status,
        source_watermark,
        last_completed_at,
        is_degraded,
    })
}

fn relationship_provenance(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<(Vec<i64>, Vec<String>)> {
    let mut stmt = conn.prepare(
        "SELECT id, source_kind FROM graph_relationship_evidence
         WHERE relationship_id = ?1 ORDER BY observed_at DESC, id DESC LIMIT 3",
    )?;
    let rows = stmt
        .query_map([id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let ids = rows.iter().map(|row| row.0).collect();
    let mut kinds: Vec<String> = rows.into_iter().map(|row| row.1).collect();
    kinds.sort();
    kinds.dedup();
    Ok((ids, kinds))
}
