//! Source- and name-lookup queries used by external-MCP resolution and eligibility checks. Kept
//! out of `repository.rs` so the base SQL module stays within its module-size ratchet.

use rusqlite::{params, Connection, OptionalExtension};

use super::mcp::MCP_SOURCE_STALE_AFTER_MILLIS;

pub fn source_kind(connection: &Connection, source_id: &str) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            "SELECT kind FROM tool_selection_sources WHERE id = ?1",
            params![source_id],
            |row| row.get(0),
        )
        .optional()
}

/// True when a source may currently serve a tool: enabled, and for an external MCP source a
/// successful sync within the freshness window. Used by describe/invoke so a residual reference
/// is refused on freshness, not only on the catalog epoch.
pub fn source_eligible(
    connection: &Connection,
    source_id: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let stale_before = now - MCP_SOURCE_STALE_AFTER_MILLIS;
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM tool_selection_sources s
                WHERE s.id = ?1 AND s.enabled = 1
                  AND (s.kind <> 'mcp_http' OR EXISTS (
                    SELECT 1 FROM tool_selection_mcp_sources ms
                     WHERE ms.source_id = s.id AND ms.last_success_at IS NOT NULL
                       AND ms.last_success_at >= ?2)))",
            params![source_id, stale_before],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value == 1)
}

/// Exact-tuple grant check used when deciding whether a config-declared grant is new or was
/// already created by another management path. An existing manual grant must never be adopted as
/// config-owned and later revoked with the source.
pub fn exact_grant_exists(
    connection: &Connection,
    principal_id: &str,
    tool_id: &str,
    scope_kind: &str,
    scope_id: &str,
) -> rusqlite::Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM tool_selection_grants
                WHERE principal_id = ?1 AND tool_id = ?2 AND scope_kind = ?3 AND scope_id = ?4)",
            params![principal_id, tool_id, scope_kind, scope_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value == 1)
}

/// Every catalog id whose display name matches. Used to refuse name-only corrections when two
/// sources publish the same tool name, instead of silently picking one with `LIMIT 1`.
pub fn tool_ids_by_name(connection: &Connection, name: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT id FROM tool_selection_catalog WHERE backend_key = ?1 ORDER BY id ASC")?;
    let rows = statement.query_map(params![name], |row| row.get::<_, String>(0))?;
    rows.collect()
}
