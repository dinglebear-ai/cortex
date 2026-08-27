//! Append-only repository observation persistence.

use super::{RepositoryObservationKind, RepositoryObservationRow};
use crate::agent_observatory::identity::event_key;
#[cfg(test)]
use crate::db::pool::DbPool;
use anyhow::{Context, Result, bail};
#[cfg(test)]
use rusqlite::TransactionBehavior;
use rusqlite::types::Type;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::str::FromStr;

const OBSERVATION_COLUMNS: &str =
    "id, observation_key, repository_id, worktree_id, observed_at, observation_kind,
     old_head_sha, new_head_sha, summary, payload_json, created_at";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryObservationInput {
    pub worktree_key: Option<String>,
    pub observation_kind: RepositoryObservationKind,
    pub new_head_sha: Option<String>,
    pub summary: String,
    pub payload_json: String,
}

fn observation_row(row: &Row<'_>) -> rusqlite::Result<RepositoryObservationRow> {
    let kind: String = row.get(5)?;
    let observation_kind = RepositoryObservationKind::from_str(&kind).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(error))
    })?;
    Ok(RepositoryObservationRow {
        id: row.get(0)?,
        observation_key: row.get(1)?,
        repository_id: row.get(2)?,
        worktree_id: row.get(3)?,
        observed_at: row.get(4)?,
        observation_kind,
        old_head_sha: row.get(6)?,
        new_head_sha: row.get(7)?,
        summary: row.get(8)?,
        payload_json: row.get(9)?,
        created_at: row.get(10)?,
    })
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_input(input: &RepositoryObservationInput) -> Result<()> {
    if input
        .worktree_key
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("worktree_key must be non-empty when present");
    }
    serde_json::from_str::<serde_json::Value>(&input.payload_json)
        .context("observation payload_json must be valid JSON")?;
    if input.observation_kind == RepositoryObservationKind::Head {
        let head = input
            .new_head_sha
            .as_deref()
            .context("head observation requires new_head_sha")?;
        if !valid_object_id(head) {
            bail!("head observation new_head_sha must be a 40- or 64-byte hex object ID");
        }
    } else if let Some(head) = input.new_head_sha.as_deref()
        && !valid_object_id(head)
    {
        bail!("new_head_sha must be a 40- or 64-byte hex object ID");
    }
    Ok(())
}

fn repository_id(tx: &Transaction<'_>, repository_key: &str) -> Result<i64> {
    tx.query_row(
        "SELECT id FROM repositories WHERE repository_key = ?1",
        [repository_key],
        |row| row.get(0),
    )
    .optional()?
    .with_context(|| format!("repository not found for key {repository_key}"))
}

fn worktree_id(
    tx: &Transaction<'_>,
    repository_id: i64,
    worktree_key: Option<&str>,
) -> Result<Option<i64>> {
    let Some(worktree_key) = worktree_key else {
        return Ok(None);
    };
    let row: Option<(i64, i64)> = tx
        .query_row(
            "SELECT id, repository_id FROM repository_worktrees WHERE worktree_key = ?1",
            [worktree_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (id, owner_repository_id) =
        row.with_context(|| format!("worktree not found for key {worktree_key}"))?;
    if owner_repository_id != repository_id {
        bail!("worktree {worktree_key} does not belong to repository");
    }
    Ok(Some(id))
}

fn latest_observation(
    tx: &Transaction<'_>,
    repository_id: i64,
    worktree_id: Option<i64>,
    kind: RepositoryObservationKind,
) -> Result<Option<RepositoryObservationRow>> {
    let sql = format!(
        "SELECT {OBSERVATION_COLUMNS}
           FROM repository_observations
          WHERE repository_id = ?1
            AND (worktree_id = ?2 OR (worktree_id IS NULL AND ?2 IS NULL))
            AND observation_kind = ?3
          ORDER BY id DESC
          LIMIT 1"
    );
    tx.query_row(
        &sql,
        params![repository_id, worktree_id, kind.as_str()],
        observation_row,
    )
    .optional()
    .context("query latest repository observation")
}

fn observation_by_key(
    tx: &Transaction<'_>,
    observation_key: &str,
) -> Result<RepositoryObservationRow> {
    let sql = format!(
        "SELECT {OBSERVATION_COLUMNS}
           FROM repository_observations
          WHERE observation_key = ?1"
    );
    tx.query_row(&sql, [observation_key], observation_row)
        .context("query inserted repository observation")
}

fn hash_component(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn deterministic_key(
    repository_key: &str,
    input: &RepositoryObservationInput,
    previous_key: Option<&str>,
    old_head_sha: Option<&str>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    for value in [
        repository_key,
        input.worktree_key.as_deref().unwrap_or(""),
        input.observation_kind.as_str(),
        previous_key.unwrap_or(""),
        old_head_sha.unwrap_or(""),
        input.new_head_sha.as_deref().unwrap_or(""),
        input.summary.as_str(),
        input.payload_json.as_str(),
    ] {
        hash_component(&mut hasher, value);
    }
    let digest = format!("{:x}", hasher.finalize());
    event_key(
        "repository_observations",
        &digest,
        input.observation_kind.as_str(),
    )
    .context("build repository observation key")
}

fn state_is_unchanged(
    latest: &RepositoryObservationRow,
    input: &RepositoryObservationInput,
) -> bool {
    if matches!(
        input.observation_kind,
        RepositoryObservationKind::WorktreeAdded | RepositoryObservationKind::WorktreeRemoved
    ) {
        return false;
    }
    if input.observation_kind == RepositoryObservationKind::Head {
        return latest.new_head_sha == input.new_head_sha;
    }
    latest.new_head_sha == input.new_head_sha
        && latest.summary == input.summary
        && latest.payload_json == input.payload_json
}

#[cfg(test)]
pub fn record_repository_observations_if_changed(
    pool: &DbPool,
    repository_key: &str,
    inputs: &[RepositoryObservationInput],
    observed_at: &str,
) -> Result<Vec<RepositoryObservationRow>> {
    validate_repository_observations(repository_key, inputs, observed_at)?;
    let mut connection = crate::db::write_conn(pool).context("acquire database connection")?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let inserted =
        record_repository_observations_if_changed_tx(&tx, repository_key, inputs, observed_at)?;
    tx.commit()?;
    Ok(inserted)
}

pub(super) fn validate_repository_observations(
    repository_key: &str,
    inputs: &[RepositoryObservationInput],
    observed_at: &str,
) -> Result<()> {
    if repository_key.trim().is_empty() {
        bail!("repository_key must be non-empty");
    }
    chrono::DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid observed_at: {observed_at}"))?;
    let mut identities = HashSet::new();
    for input in inputs {
        validate_input(input)?;
        if !identities.insert((input.worktree_key.as_deref(), input.observation_kind)) {
            bail!("duplicate worktree/kind in observation batch");
        }
    }

    Ok(())
}

pub(super) fn record_repository_observations_if_changed_tx(
    tx: &Transaction<'_>,
    repository_key: &str,
    inputs: &[RepositoryObservationInput],
    observed_at: &str,
) -> Result<Vec<RepositoryObservationRow>> {
    let repository_id = repository_id(tx, repository_key)?;
    let mut inserted = Vec::new();

    for input in inputs {
        let worktree_id = worktree_id(tx, repository_id, input.worktree_key.as_deref())?;
        let latest = latest_observation(tx, repository_id, worktree_id, input.observation_kind)?;
        if latest
            .as_ref()
            .is_some_and(|row| state_is_unchanged(row, input))
        {
            continue;
        }
        let old_head_sha = (input.observation_kind == RepositoryObservationKind::Head)
            .then(|| latest.as_ref().and_then(|row| row.new_head_sha.clone()))
            .flatten();
        let key = deterministic_key(
            repository_key,
            input,
            latest.as_ref().map(|row| row.observation_key.as_str()),
            old_head_sha.as_deref(),
        )?;
        tx.execute(
            "INSERT INTO repository_observations
                (observation_key, repository_id, worktree_id, observed_at,
                 observation_kind, old_head_sha, new_head_sha, summary, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                key,
                repository_id,
                worktree_id,
                observed_at,
                input.observation_kind.as_str(),
                old_head_sha,
                input.new_head_sha,
                input.summary,
                input.payload_json,
            ],
        )?;
        inserted.push(observation_by_key(tx, &key)?);
    }

    Ok(inserted)
}

#[cfg(test)]
pub fn list_repository_observations(
    pool: &DbPool,
    repository_id: i64,
) -> Result<Vec<RepositoryObservationRow>> {
    let connection = pool.get().context("acquire database connection")?;
    let sql = format!(
        "SELECT {OBSERVATION_COLUMNS}
           FROM repository_observations
          WHERE repository_id = ?1
          ORDER BY id"
    );
    connection
        .prepare(&sql)?
        .query_map([repository_id], observation_row)?
        .collect::<rusqlite::Result<_>>()
        .context("list repository observations")
}

#[cfg(test)]
#[path = "agent_observatory_observations_tests.rs"]
mod tests;
