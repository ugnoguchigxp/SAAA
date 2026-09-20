//! Continuation storage for MCP results larger than the 16 KiB inline envelope.
//!
//! The canonical JSON text is stored verbatim and returned as contiguous 8 KiB pages, so the
//! concatenation of every page reproduces the stored bytes exactly. Binary content stays base64
//! inside the JSON; no URL is ever fetched. A stored result is bound to the principal and run
//! scope, expires after ten minutes, and re-checks the current ACL on every read.

use rusqlite::{params, Connection, OptionalExtension};

use super::super::contracts::{now_ms, ToolSelectionError, ToolSelectionResult};
use super::repository::{self as mcp_repository, NewMcpResult};
use super::{MCP_RESULT_MAX_BYTES, MCP_RESULT_PAGE_BYTES, MCP_RESULT_TTL_MILLIS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreOutcome {
    Stored {
        result_ref: String,
        byte_count: i64,
        page_count: i64,
    },
    /// The canonical result exceeds the 1 MiB per-result bound.
    SizeLimit,
    /// The profile or run budget is exhausted even after removing expired rows.
    StorageLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultPage {
    pub text: String,
    pub page: i64,
    pub page_count: i64,
}

pub fn page_count_for(payload: &str) -> i64 {
    if payload.is_empty() {
        return 0;
    }
    let mut start = 0_usize;
    let mut pages = 0_i64;
    while start < payload.len() {
        let mut end = (start + MCP_RESULT_PAGE_BYTES).min(payload.len());
        while end > start && !payload.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            break;
        }
        start = end;
        pages += 1;
    }
    pages
}

/// Splits a stored payload into the requested UTF-8 page. Pages never split a scalar value and
/// their concatenation reproduces the stored bytes exactly.
pub fn page_text(payload: &str, page: i64) -> Option<String> {
    if page < 0 {
        return None;
    }
    let mut start = 0_usize;
    for _ in 0..page {
        if start >= payload.len() {
            return None;
        }
        let mut end = (start + MCP_RESULT_PAGE_BYTES).min(payload.len());
        while end > start && !payload.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            return None;
        }
        start = end;
    }
    if start > payload.len() {
        return None;
    }
    if start == payload.len() {
        return if page == 0 { Some(String::new()) } else { None };
    }
    let mut end = (start + MCP_RESULT_PAGE_BYTES).min(payload.len());
    while end > start && !payload.is_char_boundary(end) {
        end -= 1;
    }
    if end == start {
        return None;
    }
    Some(payload[start..end].to_string())
}

/// Stores a normalized result. Expired rows are removed first so a profile that is exactly at the
/// budget can still make progress. The caller supplies an open transaction connection.
#[allow(clippy::too_many_arguments)]
pub fn store_result(
    connection: &Connection,
    invocation_id: &str,
    principal_id: &str,
    conversation_id: &str,
    scope_key: &str,
    tool_id: &str,
    revision_id: &str,
    schema_hash: &str,
    acl_epoch: i64,
    payload_json: &str,
) -> ToolSelectionResult<StoreOutcome> {
    let byte_count = payload_json.len() as i64;
    if byte_count > MCP_RESULT_MAX_BYTES as i64 {
        return Ok(StoreOutcome::SizeLimit);
    }
    mcp_repository::purge_expired_results(connection, now_ms())
        .map_err(|_| ToolSelectionError::storage())?;
    if !mcp_repository::can_store_result(connection, principal_id, scope_key, byte_count)
        .map_err(|_| ToolSelectionError::storage())?
    {
        return Ok(StoreOutcome::StorageLimit);
    }
    let result_ref = format!("mcpres_{}", uuid::Uuid::new_v4());
    mcp_repository::insert_result(
        connection,
        &NewMcpResult {
            id: &result_ref,
            invocation_id,
            principal_id,
            conversation_id,
            scope_key,
            tool_id,
            revision_id,
            schema_hash,
            acl_epoch,
            expires_at: now_ms() + MCP_RESULT_TTL_MILLIS,
            payload_json,
            byte_count,
        },
    )
    .map_err(|_| ToolSelectionError::storage())?;
    Ok(StoreOutcome::Stored {
        result_ref,
        byte_count,
        page_count: page_count_for(payload_json),
    })
}

/// Resolves a stored page with every ownership and ACL check. A missing, expired, foreign,
/// unauthorized or out-of-range reference is reported as `not-found` so no existence is leaked.
pub fn read_page(
    connection: &Connection,
    principal_id: &str,
    project_id: Option<&str>,
    scope_key: &str,
    result_ref: &str,
    page: i64,
) -> ToolSelectionResult<ResultPage> {
    let row = mcp_repository::result(connection, result_ref)
        .map_err(|_| ToolSelectionError::storage())?
        .ok_or_else(ToolSelectionError::not_found)?;
    if row.principal_id != principal_id || row.scope_key != scope_key {
        return Err(ToolSelectionError::not_found());
    }
    if row.expires_at <= now_ms() {
        return Err(ToolSelectionError::not_found());
    }
    if !repository_grant_exists(connection, principal_id, project_id, &row.tool_id)? {
        return Err(ToolSelectionError::unauthorized());
    }
    let total = page_count_for(&row.payload_json);
    if page < 0 || page >= total {
        return Err(ToolSelectionError::not_found());
    }
    let text = page_text(&row.payload_json, page).ok_or_else(ToolSelectionError::not_found)?;
    Ok(ResultPage {
        text,
        page,
        page_count: total,
    })
}

/// Current-grant re-check for a stored result, using the same scope rules as the eligibility
/// query: the caller's user scope, or the caller's project when it was confirmed on the host.
fn repository_grant_exists(
    connection: &Connection,
    principal_id: &str,
    project_id: Option<&str>,
    tool_id: &str,
) -> ToolSelectionResult<bool> {
    let authorized = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM tool_selection_grants g
                 JOIN tool_selection_catalog c ON c.id = g.tool_id
                WHERE g.principal_id = ?1 AND g.tool_id = ?2 AND c.enabled = 1
                  AND ((g.scope_kind = 'user' AND g.scope_id = ?1)
                    OR (g.scope_kind = 'project' AND ?3 IS NOT NULL AND g.scope_id = ?3)))",
            params![principal_id, tool_id, project_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| ToolSelectionError::storage())?;
    Ok(authorized == 1)
}

/// Removes expired rows. Called at save time, at startup and from the periodic manager loop.
pub fn cleanup(connection: &Connection) -> ToolSelectionResult<usize> {
    mcp_repository::purge_expired_results(connection, now_ms())
        .map_err(|_| ToolSelectionError::storage())
}

/// Removes every stored result of a finished scope.
pub fn cleanup_scope(connection: &Connection, scope_key: &str) -> ToolSelectionResult<usize> {
    mcp_repository::purge_scope_results(connection, scope_key)
        .map_err(|_| ToolSelectionError::storage())
}

/// True when a result reference exists for this owner and scope; used only by tests and
/// management diagnostics.
pub fn exists(connection: &Connection, result_ref: &str) -> rusqlite::Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tool_selection_mcp_results WHERE id = ?1)",
            params![result_ref],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|value| value == Some(1))
}
