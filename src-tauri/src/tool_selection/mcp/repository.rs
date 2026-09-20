//! SQL access for the D4 MCP tables. Shares the single `SqliteWriter` connection like the rest
//! of the ledger; no second pool is created. Secrets (session ids, tokens, headers, raw error
//! bodies) are intentionally absent from every column.

use rusqlite::{params, Connection, OptionalExtension};

use super::super::contracts::now_ms;
use super::super::repository;
use super::{MCP_RESULT_MAX_PER_RUN, MCP_RESULT_PROFILE_MAX_BYTES, MCP_SOURCE_STALE_AFTER_MILLIS};

#[derive(Clone, Debug)]
pub struct McpSourceRow {
    pub source_id: String,
    pub config_generation: i64,
    pub endpoint_hash: String,
    pub last_success_at: Option<i64>,
    pub last_error_code: Option<String>,
    pub published_generation: i64,
}

impl McpSourceRow {
    pub fn is_fresh(&self, now: i64) -> bool {
        self.last_success_at
            .map(|at| now - at <= MCP_SOURCE_STALE_AFTER_MILLIS)
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug)]
pub struct ManagedGrant {
    pub source_id: String,
    pub principal_id: String,
    pub tool_id: String,
    pub scope_kind: String,
    pub scope_id: String,
}

#[derive(Clone, Debug)]
pub struct McpResultRow {
    pub id: String,
    pub invocation_id: String,
    pub principal_id: String,
    pub conversation_id: String,
    pub scope_key: String,
    pub tool_id: String,
    pub revision_id: String,
    pub schema_hash: String,
    pub acl_epoch: i64,
    pub expires_at: i64,
    pub payload_json: String,
    pub byte_count: i64,
}

pub struct NewMcpResult<'a> {
    pub id: &'a str,
    pub invocation_id: &'a str,
    pub principal_id: &'a str,
    pub conversation_id: &'a str,
    pub scope_key: &'a str,
    pub tool_id: &'a str,
    pub revision_id: &'a str,
    pub schema_hash: &'a str,
    pub acl_epoch: i64,
    pub expires_at: i64,
    pub payload_json: &'a str,
    pub byte_count: i64,
}

/// Registers or updates the MCP bookkeeping row for a source. The URL itself is never stored;
/// only the normalized endpoint hash.
pub fn upsert_source(
    connection: &Connection,
    source_id: &str,
    config_generation: i64,
    endpoint_hash: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_mcp_sources(
           source_id, config_generation, endpoint_hash, last_success_at, last_error_code,
           published_generation)
         VALUES (?1, ?2, ?3, NULL, NULL, 0)
         ON CONFLICT(source_id) DO UPDATE SET
           config_generation = excluded.config_generation,
           endpoint_hash = excluded.endpoint_hash",
        params![source_id, config_generation, endpoint_hash],
    )?;
    Ok(())
}

pub fn source(connection: &Connection, source_id: &str) -> rusqlite::Result<Option<McpSourceRow>> {
    connection
        .query_row(
            "SELECT source_id, config_generation, endpoint_hash, last_success_at, last_error_code,
                    published_generation
               FROM tool_selection_mcp_sources WHERE source_id = ?1",
            params![source_id],
            |row| {
                Ok(McpSourceRow {
                    source_id: row.get(0)?,
                    config_generation: row.get(1)?,
                    endpoint_hash: row.get(2)?,
                    last_success_at: row.get(3)?,
                    last_error_code: row.get(4)?,
                    published_generation: row.get(5)?,
                })
            },
        )
        .optional()
}

pub fn mark_success(
    connection: &Connection,
    source_id: &str,
    at: i64,
    published_generation: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE tool_selection_mcp_sources
            SET last_success_at = ?2, last_error_code = NULL, published_generation = ?3
          WHERE source_id = ?1",
        params![source_id, at, published_generation],
    )?;
    Ok(())
}

pub fn mark_error(
    connection: &Connection,
    source_id: &str,
    error_code: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE tool_selection_mcp_sources SET last_error_code = ?2 WHERE source_id = ?1",
        params![source_id, error_code],
    )?;
    Ok(())
}

pub fn is_fresh(connection: &Connection, source_id: &str, now: i64) -> rusqlite::Result<bool> {
    Ok(source(connection, source_id)?
        .map(|row| row.is_fresh(now))
        .unwrap_or(false))
}

pub fn managed_grants_for_source(
    connection: &Connection,
    source_id: &str,
) -> rusqlite::Result<Vec<ManagedGrant>> {
    let mut statement = connection.prepare(
        "SELECT source_id, principal_id, tool_id, scope_kind, scope_id
           FROM tool_selection_mcp_managed_grants WHERE source_id = ?1",
    )?;
    let rows = statement.query_map(params![source_id], |row| {
        Ok(ManagedGrant {
            source_id: row.get(0)?,
            principal_id: row.get(1)?,
            tool_id: row.get(2)?,
            scope_kind: row.get(3)?,
            scope_id: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn managed_grant_exists(
    connection: &Connection,
    source_id: &str,
    principal_id: &str,
    tool_id: &str,
    scope_kind: &str,
    scope_id: &str,
) -> rusqlite::Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM tool_selection_mcp_managed_grants
                WHERE source_id = ?1 AND principal_id = ?2 AND tool_id = ?3
                  AND scope_kind = ?4 AND scope_id = ?5)",
            params![source_id, principal_id, tool_id, scope_kind, scope_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value == 1)
}

pub fn insert_managed_grant(
    connection: &Connection,
    grant: &ManagedGrant,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO tool_selection_mcp_managed_grants(
           source_id, principal_id, tool_id, scope_kind, scope_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            grant.source_id,
            grant.principal_id,
            grant.tool_id,
            grant.scope_kind,
            grant.scope_id
        ],
    )?;
    Ok(())
}

/// Removes one managed grant and the actual authorization only when this row owned it. A manual
/// grant for the same tuple is never deleted.
pub fn delete_managed_grant(
    connection: &Connection,
    grant: &ManagedGrant,
) -> rusqlite::Result<bool> {
    let removed = connection.execute(
        "DELETE FROM tool_selection_mcp_managed_grants
          WHERE source_id = ?1 AND principal_id = ?2 AND tool_id = ?3
            AND scope_kind = ?4 AND scope_id = ?5",
        params![
            grant.source_id,
            grant.principal_id,
            grant.tool_id,
            grant.scope_kind,
            grant.scope_id
        ],
    )?;
    if removed == 1 {
        connection.execute(
            "DELETE FROM tool_selection_grants
              WHERE principal_id = ?1 AND tool_id = ?2 AND scope_kind = ?3 AND scope_id = ?4",
            params![
                grant.principal_id,
                grant.tool_id,
                grant.scope_kind,
                grant.scope_id
            ],
        )?;
    }
    Ok(removed == 1)
}

pub fn insert_result(
    connection: &Connection,
    row: &NewMcpResult<'_>,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO tool_selection_mcp_results(
           id, invocation_id, principal_id, conversation_id, scope_key, tool_id, revision_id,
           schema_hash, acl_epoch, expires_at, payload_json, byte_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            row.id,
            row.invocation_id,
            row.principal_id,
            row.conversation_id,
            row.scope_key,
            row.tool_id,
            row.revision_id,
            row.schema_hash,
            row.acl_epoch,
            row.expires_at,
            row.payload_json,
            row.byte_count
        ],
    )?;
    Ok(())
}

pub fn result(connection: &Connection, id: &str) -> rusqlite::Result<Option<McpResultRow>> {
    connection
        .query_row(
            "SELECT id, invocation_id, principal_id, conversation_id, scope_key, tool_id,
                    revision_id, schema_hash, acl_epoch, expires_at, payload_json, byte_count
               FROM tool_selection_mcp_results WHERE id = ?1",
            params![id],
            |row| {
                Ok(McpResultRow {
                    id: row.get(0)?,
                    invocation_id: row.get(1)?,
                    principal_id: row.get(2)?,
                    conversation_id: row.get(3)?,
                    scope_key: row.get(4)?,
                    tool_id: row.get(5)?,
                    revision_id: row.get(6)?,
                    schema_hash: row.get(7)?,
                    acl_epoch: row.get(8)?,
                    expires_at: row.get(9)?,
                    payload_json: row.get(10)?,
                    byte_count: row.get(11)?,
                })
            },
        )
        .optional()
}

pub fn scope_result_bytes(
    connection: &Connection,
    principal_id: &str,
    scope_key: &str,
) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COALESCE(SUM(byte_count), 0) FROM tool_selection_mcp_results
          WHERE principal_id = ?1 AND scope_key = ?2",
        params![principal_id, scope_key],
        |row| row.get(0),
    )
}

pub fn scope_result_count(
    connection: &Connection,
    principal_id: &str,
    scope_key: &str,
) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM tool_selection_mcp_results
          WHERE principal_id = ?1 AND scope_key = ?2",
        params![principal_id, scope_key],
        |row| row.get(0),
    )
}

/// True when one more result of `byte_count` bytes still fits the profile and run budgets. The
/// profile budget is summed over every run of the principal.
pub fn can_store_result(
    connection: &Connection,
    principal_id: &str,
    scope_key: &str,
    byte_count: i64,
) -> rusqlite::Result<bool> {
    let profile: i64 = connection.query_row(
        "SELECT COALESCE(SUM(byte_count), 0) FROM tool_selection_mcp_results WHERE principal_id = ?1",
        params![principal_id],
        |row| row.get(0),
    )?;
    if profile + byte_count > MCP_RESULT_PROFILE_MAX_BYTES as i64 {
        return Ok(false);
    }
    if scope_result_count(connection, principal_id, scope_key)? >= MCP_RESULT_MAX_PER_RUN as i64 {
        return Ok(false);
    }
    Ok(true)
}

pub fn delete_result(connection: &Connection, id: &str) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM tool_selection_mcp_results WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

pub fn purge_expired_results(connection: &Connection, now: i64) -> rusqlite::Result<usize> {
    connection.execute(
        "DELETE FROM tool_selection_mcp_results WHERE expires_at <= ?1",
        params![now],
    )
}

/// Removes results whose owning invocation has finished and whose run scope is no longer the
/// caller's active scope. Bounded cleanup used at startup and on shutdown.
pub fn purge_scope_results(connection: &Connection, scope_key: &str) -> rusqlite::Result<usize> {
    connection.execute(
        "DELETE FROM tool_selection_mcp_results WHERE scope_key = ?1",
        params![scope_key],
    )
}

/// Number of MCP tools currently published by a source. Used to enforce the per-profile total.
pub fn published_tool_count(connection: &Connection, source_id: &str) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM tool_selection_catalog WHERE source_id = ?1",
        params![source_id],
        |row| row.get(0),
    )
}

/// Enables or disables every tool of a source without deleting revision history.
pub fn set_source_tools_enabled(
    connection: &Connection,
    source_id: &str,
    enabled: bool,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE tool_selection_catalog SET enabled = ?2 WHERE source_id = ?1",
        params![source_id, enabled as i64],
    )
}

/// A tool is a re-appearance when the catalog row exists but is disabled. Its stable tool id must
/// stay the same.
pub fn tool_enabled(connection: &Connection, tool_id: &str) -> rusqlite::Result<Option<bool>> {
    connection
        .query_row(
            "SELECT enabled FROM tool_selection_catalog WHERE id = ?1",
            params![tool_id],
            |row| row.get::<_, i64>(0).map(|value| value == 1),
        )
        .optional()
}

/// Convenience for callers that only need "fresh right now".
pub fn fresh_now(connection: &Connection, source_id: &str) -> rusqlite::Result<bool> {
    is_fresh(connection, source_id, now_ms())
}

/// Marks a source's ledger row disabled as part of config removal. The revision history remains.
pub fn disable_source(
    connection: &Connection,
    source_id: &str,
) -> rusqlite::Result<usize> {
    repository::set_source_enabled(connection, source_id, false)
}
