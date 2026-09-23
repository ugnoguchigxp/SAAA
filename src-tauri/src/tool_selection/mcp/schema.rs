//! D4 schema additions and the version-24 `tool_selection_sources.kind` rebuild. Kept in the MCP
//! module so the base tool-selection schema file stays close to its pre-D4 size.

use rusqlite::Connection;

/// Creates the D4 MCP tables. Idempotent.
pub fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_selection_mcp_sources (
           source_id TEXT PRIMARY KEY,
           config_generation INTEGER NOT NULL CHECK(config_generation >= 0),
           endpoint_hash TEXT NOT NULL CHECK(length(endpoint_hash) = 64),
           last_success_at INTEGER,
           last_error_code TEXT CHECK(last_error_code IS NULL OR length(last_error_code) <= 64),
           published_generation INTEGER NOT NULL CHECK(published_generation >= 0),
           FOREIGN KEY(source_id) REFERENCES tool_selection_sources(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS tool_selection_mcp_managed_grants (
           source_id TEXT NOT NULL,
           principal_id TEXT NOT NULL CHECK(length(principal_id) BETWEEN 1 AND 160),
           tool_id TEXT NOT NULL,
           scope_kind TEXT NOT NULL CHECK(scope_kind IN ('user', 'project')),
           scope_id TEXT NOT NULL CHECK(length(scope_id) BETWEEN 1 AND 160),
           PRIMARY KEY(source_id, principal_id, tool_id, scope_kind, scope_id),
           FOREIGN KEY(source_id) REFERENCES tool_selection_sources(id) ON DELETE CASCADE,
           FOREIGN KEY(tool_id) REFERENCES tool_selection_catalog(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_mcp_managed_grants_tool
           ON tool_selection_mcp_managed_grants(source_id, tool_id);
         CREATE TABLE IF NOT EXISTS tool_selection_mcp_results (
           id TEXT PRIMARY KEY,
           invocation_id TEXT NOT NULL,
           principal_id TEXT NOT NULL CHECK(length(principal_id) BETWEEN 1 AND 160),
           conversation_id TEXT NOT NULL,
           scope_key TEXT NOT NULL CHECK(length(scope_key) BETWEEN 1 AND 200),
           tool_id TEXT NOT NULL,
           revision_id TEXT NOT NULL,
           schema_hash TEXT NOT NULL CHECK(length(schema_hash) = 64),
           acl_epoch INTEGER NOT NULL CHECK(acl_epoch >= 0),
           expires_at INTEGER NOT NULL,
           payload_json TEXT NOT NULL CHECK(json_valid(payload_json) AND length(payload_json) <= 1048576),
           byte_count INTEGER NOT NULL CHECK(byte_count > 0 AND byte_count <= 1048576),
           FOREIGN KEY(invocation_id) REFERENCES tool_selection_invocations(id) ON DELETE CASCADE,
           FOREIGN KEY(revision_id) REFERENCES tool_selection_revisions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_mcp_results_scope
           ON tool_selection_mcp_results(principal_id, scope_key);
         CREATE INDEX IF NOT EXISTS idx_tool_selection_mcp_results_expiry
           ON tool_selection_mcp_results(expires_at);
         CREATE TABLE IF NOT EXISTS tool_selection_rule_source_bindings (
           rule_id TEXT NOT NULL,
           tool_id TEXT NOT NULL,
           endpoint_hash TEXT NOT NULL CHECK(length(endpoint_hash) <= 64),
           PRIMARY KEY(rule_id, tool_id),
           FOREIGN KEY(rule_id) REFERENCES tool_selection_rules(id) ON DELETE CASCADE,
           FOREIGN KEY(tool_id) REFERENCES tool_selection_catalog(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_rule_source_bindings_tool
           ON tool_selection_rule_source_bindings(tool_id);",
    )?;
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('tool_selection_mcp_results') WHERE name='record_id')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        connection
            .execute_batch("ALTER TABLE tool_selection_mcp_results ADD COLUMN record_id TEXT")?;
    }
    Ok(())
}

/// Version-25 migration. A correction rule that names a remote MCP tool is only applicable to the
/// endpoint it was learned on. Existing rules predate endpoint capture, so their bindings are
/// created empty and therefore unconfirmed: they are not silently reattached to whatever endpoint
/// is current, and an operator must re-create the correction to confirm a new binding.
///
/// This must run inside the schema transaction after `migrate`, and only when the database is
/// upgraded from a version older than 25. It is idempotent because of `INSERT OR IGNORE`.
pub fn backfill_rule_source_bindings(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "INSERT OR IGNORE INTO tool_selection_rule_source_bindings(rule_id, tool_id, endpoint_hash)
           SELECT r.id, r.target_tool_id, ''
             FROM tool_selection_rules r
             JOIN tool_selection_catalog c ON c.id = r.target_tool_id
             JOIN tool_selection_sources s ON s.id = c.source_id
            WHERE s.kind = 'mcp_http' AND r.target_tool_id IS NOT NULL;
         INSERT OR IGNORE INTO tool_selection_rule_source_bindings(rule_id, tool_id, endpoint_hash)
           SELECT r.id, r.preferred_tool_id, ''
             FROM tool_selection_rules r
             JOIN tool_selection_catalog c ON c.id = r.preferred_tool_id
             JOIN tool_selection_sources s ON s.id = c.source_id
            WHERE s.kind = 'mcp_http' AND r.preferred_tool_id IS NOT NULL;",
    )
}

/// Widens `tool_selection_sources.kind` to accept the current local and remote backends. `CREATE TABLE IF NOT EXISTS`
/// cannot change an existing CHECK constraint, so a database created before version 24 is
/// rebuilt. The rebuild swaps a parent table referenced by the catalog, which is only legal with
/// foreign keys disabled; the caller therefore invokes this before the main schema transaction
/// and this wrapper toggles the pragma itself. Data and referencing rows are preserved.
pub fn migrate_sources_kind(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let result = migrate_sources_kind_inner(connection);
    let restored = connection.execute_batch("PRAGMA foreign_keys = ON;");
    result.and(restored)
}

fn migrate_sources_kind_inner(connection: &Connection) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tool_selection_sources')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    let sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='tool_selection_sources'",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(sql) = sql else {
        return Ok(());
    };
    if sql.contains("artifact_webview") {
        return Ok(());
    }
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE tool_selection_sources_new (
           id TEXT PRIMARY KEY,
           kind TEXT NOT NULL CHECK(kind IN ('llang', 'mcp_http', 'artifact_webview')),
           owner_principal TEXT NOT NULL CHECK(length(owner_principal) BETWEEN 1 AND 160),
           enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
         );
         INSERT INTO tool_selection_sources_new(id, kind, owner_principal, enabled)
           SELECT id, kind, owner_principal, enabled FROM tool_selection_sources;
         DROP TABLE tool_selection_sources;
         ALTER TABLE tool_selection_sources_new RENAME TO tool_selection_sources;",
    )?;
    transaction.commit()
}
