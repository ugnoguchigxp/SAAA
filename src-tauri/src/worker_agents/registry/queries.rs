//! Read side of the registry: summaries for IPC, task and blocklist listings.
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::read_meta;
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn revision_summary(
    connection: &Connection,
    revision_id: &str,
) -> Result<RevisionSummary, String> {
    let row = connection
        .query_row(
            "SELECT revision, review_state, purpose, definition_hash, output_kind, created_at_ms
             FROM worker_profile_revisions WHERE id = ?1",
            params![revision_id],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "worker revision not found".to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT tool_key FROM worker_profile_tools WHERE profile_revision_id = ?1 ORDER BY ordinal",
        )
        .map_err(|error| error.to_string())?;
    let tool_keys = statement
        .query_map(params![revision_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(RevisionSummary {
        revision_id: revision_id.to_string(),
        revision: row.0,
        review_state: ReviewState::parse(&row.1).ok_or("worker revision has an invalid state")?,
        purpose: row.2,
        definition_hash: row.3,
        tool_keys,
        output_kind: OutputKind::parse(&row.4).ok_or("worker revision has an invalid kind")?,
        created_at_ms: row.5,
    })
}

struct ProfileRow {
    origin: String,
    enabled: bool,
    pinned_offer: bool,
    current_revision_id: Option<String>,
}

fn profile_row(connection: &Connection, profile_id: &str) -> Result<ProfileRow, String> {
    connection
        .query_row(
            "SELECT origin, enabled, pinned_offer, current_revision_id FROM worker_profiles WHERE id = ?1",
            params![profile_id],
            |row| {
                Ok(ProfileRow {
                    origin: row.get(0)?,
                    enabled: row.get::<_, i64>(1)? != 0,
                    pinned_offer: row.get::<_, i64>(2)? != 0,
                    current_revision_id: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "worker agent not found".to_string())
}

fn revision_ids(
    connection: &Connection,
    profile_id: &str,
    only_state: Option<ReviewState>,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id FROM worker_profile_revisions
             WHERE profile_id = ?1 AND (?2 IS NULL OR review_state = ?2) ORDER BY revision DESC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![profile_id, only_state.map(ReviewState::as_str)],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub(crate) fn get_summary(
    connection: &Connection,
    profile_id: &str,
) -> Result<WorkerAgentSummary, String> {
    let profile = profile_row(connection, profile_id)?;
    let current = profile
        .current_revision_id
        .as_deref()
        .map(|id| revision_summary(connection, id))
        .transpose()?;
    let draft = revision_ids(connection, profile_id, Some(ReviewState::Draft))?
        .first()
        .map(|id| revision_summary(connection, id))
        .transpose()?;
    Ok(WorkerAgentSummary {
        profile_id: profile_id.to_string(),
        origin: profile.origin,
        enabled: profile.enabled,
        pinned_offer: profile.pinned_offer,
        current,
        draft,
    })
}

/// Current and latest-draft summaries of every profile. A draft is never reported as current.
pub(crate) fn list_agents(connection: &Connection) -> Result<Vec<WorkerAgentSummary>, String> {
    let mut statement = connection
        .prepare("SELECT id FROM worker_profiles ORDER BY pinned_offer DESC, id")
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    ids.iter().map(|id| get_summary(connection, id)).collect()
}

pub(crate) fn get_agent(
    connection: &Connection,
    profile_id: &str,
) -> Result<WorkerAgentDetail, String> {
    let profile = profile_row(connection, profile_id)?;
    let revisions = revision_ids(connection, profile_id, None)?
        .iter()
        .map(|id| revision_summary(connection, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WorkerAgentDetail {
        profile_id: profile_id.to_string(),
        origin: profile.origin,
        enabled: profile.enabled,
        pinned_offer: profile.pinned_offer,
        revisions,
    })
}

pub(crate) fn get_web_search_mode(connection: &Connection) -> Result<WebSearchMode, String> {
    Ok(read_meta(connection)?.web_search_mode)
}

/// Recent tasks, newest first. Never selects `input_json`/`result_json`.
pub(crate) fn list_tasks(
    connection: &Connection,
    limit: u32,
) -> Result<Vec<WorkerTaskSummary>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, profile_id, state, delivery, failure_code, created_at_ms, updated_at_ms
             FROM worker_tasks ORDER BY created_at_ms DESC, id LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![limit.clamp(1, 100)], |row| {
            Ok(WorkerTaskSummary {
                task_id: row.get(0)?,
                profile_id: row.get(1)?,
                state: row.get(2)?,
                delivery: row.get(3)?,
                failure_code: row.get(4)?,
                created_at_ms: row.get(5)?,
                updated_at_ms: row.get(6)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// Newest first, then `url_hash`. `after` is the `url_hash` of the last entry already seen; an
/// unknown cursor (for example a row removed meanwhile) yields an empty page.
pub(crate) fn list_blocklist(
    connection: &Connection,
    limit: u32,
    after: Option<&str>,
) -> Result<Vec<BlocklistEntry>, String> {
    let limit = limit.clamp(1, 200);
    let cursor = match after {
        None => None,
        Some(hash) => match connection
            .query_row(
                "SELECT created_at_ms FROM worker_url_blocklist WHERE url_hash = ?1",
                params![hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
        {
            Some(created) => Some((created, hash.to_string())),
            None => return Ok(Vec::new()),
        },
    };
    let (created, hash) = cursor.unwrap_or((i64::MAX, String::new()));
    let mut statement = connection
        .prepare(
            "SELECT url_hash, host, reason, created_at_ms FROM worker_url_blocklist
             WHERE ?3 = 0 OR created_at_ms < ?1 OR (created_at_ms = ?1 AND url_hash > ?2)
             ORDER BY created_at_ms DESC, url_hash LIMIT ?4",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![created, hash, after.is_some() as i64, limit],
            |row| {
                Ok(BlocklistEntry {
                    url_hash: row.get(0)?,
                    host: row.get(1)?,
                    reason: row.get(2)?,
                    created_at_ms: row.get(3)?,
                })
            },
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// Blocklist rows have no expiry: this is the only deletion path.
pub(crate) fn remove_blocklist(connection: &Connection, url_hash: &str) -> Result<bool, String> {
    let removed = connection
        .execute(
            "DELETE FROM worker_url_blocklist WHERE url_hash = ?1",
            params![url_hash],
        )
        .map_err(|error| error.to_string())?;
    Ok(removed > 0)
}
