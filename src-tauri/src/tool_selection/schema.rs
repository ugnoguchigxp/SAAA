//! SQLite schema for the tool-selection ledger. Called from the existing migration transaction so
//! the ledger shares the single writer and the existing backup/journal path.

use rusqlite::Connection;

/// Creates the tool-selection tables, indexes and FTS5 index. Idempotent, so it can run on an
/// empty database, an upgraded database, or a second time after a failed migration.
pub fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_selection_meta (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           catalog_epoch INTEGER NOT NULL DEFAULT 0 CHECK(catalog_epoch >= 0),
           acl_epoch INTEGER NOT NULL DEFAULT 0 CHECK(acl_epoch >= 0),
           rule_epoch INTEGER NOT NULL DEFAULT 0 CHECK(rule_epoch >= 0)
         );
         INSERT OR IGNORE INTO tool_selection_meta(singleton, catalog_epoch, acl_epoch, rule_epoch)
           VALUES (1, 0, 0, 0);
         CREATE TABLE IF NOT EXISTS tool_selection_sources (
           id TEXT PRIMARY KEY,
           kind TEXT NOT NULL CHECK(kind IN ('llang', 'mcp_http')),
           owner_principal TEXT NOT NULL CHECK(length(owner_principal) BETWEEN 1 AND 160),
           enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
         );
         CREATE TABLE IF NOT EXISTS tool_selection_catalog (
           id TEXT PRIMARY KEY,
           source_id TEXT NOT NULL,
           backend_key TEXT NOT NULL CHECK(length(backend_key) BETWEEN 1 AND 256),
           current_revision_id TEXT,
           enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
           UNIQUE(source_id, backend_key),
           FOREIGN KEY(source_id) REFERENCES tool_selection_sources(id),
           FOREIGN KEY(id, current_revision_id)
             REFERENCES tool_selection_revisions(tool_id, id)
             DEFERRABLE INITIALLY DEFERRED
         );
         CREATE TABLE IF NOT EXISTS tool_selection_revisions (
           id TEXT PRIMARY KEY,
           tool_id TEXT NOT NULL,
           schema_hash TEXT NOT NULL CHECK(length(schema_hash) = 64),
           description_hash TEXT NOT NULL CHECK(length(description_hash) = 64),
           input_schema_json TEXT NOT NULL CHECK(json_valid(input_schema_json)),
           output_schema_json TEXT CHECK(output_schema_json IS NULL OR json_valid(output_schema_json)),
           search_text TEXT NOT NULL CHECK(length(search_text) <= 65536),
           operations_json TEXT NOT NULL CHECK(json_valid(operations_json)),
           objects_json TEXT NOT NULL CHECK(json_valid(objects_json)),
           effect TEXT NOT NULL CHECK(effect IN ('pure', 'read', 'write', 'unknown')),
           backend_binding_json TEXT NOT NULL CHECK(json_valid(backend_binding_json)),
           created_at INTEGER NOT NULL,
           UNIQUE(tool_id, id),
           FOREIGN KEY(tool_id) REFERENCES tool_selection_catalog(id)
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_revisions_tool
           ON tool_selection_revisions(tool_id, created_at DESC);
         CREATE TABLE IF NOT EXISTS tool_selection_usage_pages (
           revision_id TEXT NOT NULL,
           section TEXT NOT NULL CHECK(section IN ('usage', 'examples', 'troubleshooting')),
           page INTEGER NOT NULL CHECK(page >= 0),
           text TEXT NOT NULL CHECK(length(text) <= 8192),
           PRIMARY KEY(revision_id, section, page),
           FOREIGN KEY(revision_id) REFERENCES tool_selection_revisions(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS tool_selection_grants (
           principal_id TEXT NOT NULL CHECK(length(principal_id) BETWEEN 1 AND 160),
           tool_id TEXT NOT NULL,
           scope_kind TEXT NOT NULL CHECK(scope_kind IN ('user', 'project')),
           scope_id TEXT NOT NULL CHECK(length(scope_id) BETWEEN 1 AND 160),
           PRIMARY KEY(principal_id, tool_id, scope_kind, scope_id),
           FOREIGN KEY(tool_id) REFERENCES tool_selection_catalog(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS tool_selection_embeddings (
           revision_id TEXT NOT NULL,
           model_hash TEXT NOT NULL CHECK(length(model_hash) BETWEEN 1 AND 128),
           dimension INTEGER NOT NULL CHECK(dimension > 0),
           vector BLOB NOT NULL,
           PRIMARY KEY(revision_id, model_hash),
           CHECK(length(vector) = 4 * dimension),
           FOREIGN KEY(revision_id) REFERENCES tool_selection_revisions(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS tool_selection_decisions (
           id TEXT PRIMARY KEY,
           principal_id TEXT NOT NULL,
           conversation_id TEXT NOT NULL,
           run_id TEXT,
           message_id TEXT,
           scenario_json TEXT NOT NULL CHECK(json_valid(scenario_json)),
           catalog_epoch INTEGER NOT NULL,
           acl_epoch INTEGER NOT NULL,
           rule_epoch INTEGER NOT NULL,
           model_hash TEXT,
           status TEXT NOT NULL CHECK(status IN ('ok', 'no_match', 'degraded')),
           created_at INTEGER NOT NULL,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
           FOREIGN KEY(message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_decisions_scope
           ON tool_selection_decisions(principal_id, conversation_id, created_at DESC);
         CREATE TABLE IF NOT EXISTS tool_selection_candidates (
           decision_id TEXT NOT NULL,
           revision_id TEXT NOT NULL,
           lex_rank INTEGER,
           vec_rank INTEGER,
           raw_score REAL,
           base_score REAL NOT NULL,
           final_score REAL NOT NULL,
           rule_ids_json TEXT NOT NULL CHECK(json_valid(rule_ids_json)),
           final_rank INTEGER NOT NULL CHECK(final_rank >= 0),
           PRIMARY KEY(decision_id, revision_id),
           FOREIGN KEY(decision_id) REFERENCES tool_selection_decisions(id) ON DELETE CASCADE,
           FOREIGN KEY(revision_id) REFERENCES tool_selection_revisions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_candidates_decision
           ON tool_selection_candidates(decision_id, final_rank);
         CREATE TABLE IF NOT EXISTS tool_selection_invocations (
           id TEXT PRIMARY KEY,
           decision_id TEXT,
           revision_id TEXT NOT NULL,
           backend_call_id TEXT,
           technical_status TEXT NOT NULL CHECK(technical_status IN
             ('running', 'succeeded', 'failed', 'cancelled', 'unknown', 'interrupted')),
           satisfaction TEXT NOT NULL DEFAULT 'unknown' CHECK(satisfaction IN
             ('unknown', 'explicit_positive', 'explicit_negative')),
           started_at INTEGER,
           finished_at INTEGER,
           error_code TEXT,
           FOREIGN KEY(decision_id) REFERENCES tool_selection_decisions(id) ON DELETE CASCADE,
           FOREIGN KEY(revision_id) REFERENCES tool_selection_revisions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_invocations_decision
           ON tool_selection_invocations(decision_id, started_at DESC);
         CREATE TABLE IF NOT EXISTS tool_selection_feedback (
           id TEXT PRIMARY KEY,
           principal_id TEXT NOT NULL,
           message_id TEXT NOT NULL,
           decision_id TEXT,
           kind TEXT NOT NULL CHECK(kind IN
             ('tool_choice', 'arguments', 'output_quality', 'source_scope',
              'temporary_constraint', 'explicit_positive', 'revoke', 'ambiguous', 'unknown')),
           evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)),
           proposal_json TEXT NOT NULL CHECK(json_valid(proposal_json)),
           status TEXT NOT NULL CHECK(status IN
             ('pending', 'ambiguous', 'applied', 'rejected', 'revoked')),
           idempotency_key TEXT NOT NULL UNIQUE,
           created_at INTEGER NOT NULL,
           FOREIGN KEY(message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE,
           FOREIGN KEY(decision_id) REFERENCES tool_selection_decisions(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_feedback_message
           ON tool_selection_feedback(message_id);
         CREATE TABLE IF NOT EXISTS tool_selection_rules (
           id TEXT PRIMARY KEY,
           feedback_id TEXT NOT NULL,
           principal_id TEXT NOT NULL,
           scope_kind TEXT NOT NULL CHECK(scope_kind IN ('user', 'project', 'task', 'conversation')),
           scope_id TEXT NOT NULL,
           operation TEXT NOT NULL,
           object_type TEXT NOT NULL,
           phase TEXT,
           input_kind TEXT,
           source_constraint TEXT,
           target_tool_id TEXT,
           target_revision_id TEXT,
           preferred_tool_id TEXT,
           action TEXT NOT NULL CHECK(action IN ('avoid', 'prefer', 'pairwise', 'forbid')),
           strength REAL NOT NULL,
           expires_at INTEGER,
           state TEXT NOT NULL CHECK(state IN ('active', 'revoked', 'superseded')),
           created_at INTEGER NOT NULL,
           FOREIGN KEY(feedback_id) REFERENCES tool_selection_feedback(id) ON DELETE CASCADE,
           FOREIGN KEY(target_tool_id) REFERENCES tool_selection_catalog(id),
           FOREIGN KEY(preferred_tool_id) REFERENCES tool_selection_catalog(id)
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_rules_scope
           ON tool_selection_rules(principal_id, state, scope_kind, scope_id);
         CREATE INDEX IF NOT EXISTS idx_tool_selection_rules_feedback
           ON tool_selection_rules(feedback_id);
         CREATE VIRTUAL TABLE IF NOT EXISTS tool_selection_fts USING fts5(
           revision_id UNINDEXED,
           search_text,
           tokenize = 'trigram'
         );
         CREATE TABLE IF NOT EXISTS tool_selection_mcp_sources (
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
           payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
           byte_count INTEGER NOT NULL CHECK(byte_count > 0 AND byte_count <= 1048576),
           FOREIGN KEY(invocation_id) REFERENCES tool_selection_invocations(id) ON DELETE CASCADE,
           FOREIGN KEY(revision_id) REFERENCES tool_selection_revisions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_tool_selection_mcp_results_scope
           ON tool_selection_mcp_results(principal_id, scope_key);
         CREATE INDEX IF NOT EXISTS idx_tool_selection_mcp_results_expiry
           ON tool_selection_mcp_results(expires_at);",
    )
}

/// Widens `tool_selection_sources.kind` to accept `mcp_http`. `CREATE TABLE IF NOT EXISTS`
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
    if sql.contains("mcp_http") {
        return Ok(());
    }
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE tool_selection_sources_new (
           id TEXT PRIMARY KEY,
           kind TEXT NOT NULL CHECK(kind IN ('llang', 'mcp_http')),
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
