use rusqlite::Connection;

/// Additive v19-v20 schema. Provider request bodies and context text are never stored here.
pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS context_generations (
           id TEXT PRIMARY KEY,
           run_id TEXT NOT NULL REFERENCES runtime_runs(id) ON DELETE CASCADE,
           provider_session_id TEXT REFERENCES provider_sessions(id) ON DELETE SET NULL,
           provider_id TEXT NOT NULL,
           ordinal INTEGER NOT NULL CHECK(ordinal >= 1),
           purpose TEXT NOT NULL CHECK(length(purpose) BETWEEN 1 AND 64),
           envelope_digest TEXT NOT NULL CHECK(length(envelope_digest) = 64),
           request_digest TEXT NOT NULL CHECK(length(request_digest) = 64),
           projected_bytes INTEGER NOT NULL CHECK(projected_bytes >= 0),
           current_instruction_count INTEGER NOT NULL CHECK(current_instruction_count >= 0),
           health_status TEXT NOT NULL CHECK(health_status IN ('green','yellow','red')),
           status TEXT NOT NULL CHECK(status IN ('planned','dispatched','completed','failed','cancelled','interrupted')),
           failure_kind TEXT,
           started_at TEXT NOT NULL,
           completed_at TEXT,
           UNIQUE(run_id, ordinal)
         );
         CREATE INDEX IF NOT EXISTS idx_context_generations_run_status
           ON context_generations(run_id, status, ordinal);
         CREATE TABLE IF NOT EXISTS context_generation_inputs (
           generation_id TEXT NOT NULL REFERENCES context_generations(id) ON DELETE CASCADE,
           source_kind TEXT NOT NULL CHECK(length(source_kind) BETWEEN 1 AND 64),
           source_id TEXT NOT NULL,
           source_version INTEGER NOT NULL CHECK(source_version >= 1),
           source_digest TEXT NOT NULL CHECK(length(source_digest) = 64),
           requirement TEXT NOT NULL CHECK(requirement IN ('must','should','may')),
           placement TEXT NOT NULL CHECK(placement IN ('base','view','tool-schema','reference')),
           selected INTEGER NOT NULL CHECK(selected IN (0,1)),
           omission_reason TEXT,
           metadata_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(metadata_json)),
           PRIMARY KEY(generation_id, source_kind, source_id, source_version)
         );
         CREATE INDEX IF NOT EXISTS idx_context_generation_inputs_source
           ON context_generation_inputs(source_kind, source_id, source_version);
         CREATE TABLE IF NOT EXISTS context_scopes (
           scope_key TEXT PRIMARY KEY,
           kind TEXT NOT NULL CHECK(kind IN ('user','project','task','resource','request')),
           opaque_id TEXT NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('active','revoked')),
           created_at TEXT NOT NULL,
           UNIQUE(kind, opaque_id)
         );
         CREATE TABLE IF NOT EXISTS context_scope_links (
           parent_scope_key TEXT NOT NULL REFERENCES context_scopes(scope_key),
           child_scope_key TEXT NOT NULL REFERENCES context_scopes(scope_key),
           relation TEXT NOT NULL CHECK(relation IN ('owns','parent')),
           created_at TEXT NOT NULL,
           PRIMARY KEY(parent_scope_key,child_scope_key,relation),
           CHECK(parent_scope_key != child_scope_key)
         );
         CREATE TABLE IF NOT EXISTS context_scope_epochs (
           scope_key TEXT PRIMARY KEY REFERENCES context_scopes(scope_key) ON DELETE CASCADE,
           epoch INTEGER NOT NULL DEFAULT 0 CHECK(epoch >= 0)
         );
         CREATE TABLE IF NOT EXISTS runtime_scope_resolutions (
           run_id TEXT PRIMARY KEY REFERENCES runtime_runs(id) ON DELETE CASCADE,
           status TEXT NOT NULL CHECK(status IN ('resolved','ambiguous','missing')),
           focus_scope_key TEXT REFERENCES context_scopes(scope_key),
           scope_digest TEXT NOT NULL CHECK(length(scope_digest)=64),
           reason_code TEXT,
           resolved_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runtime_run_scopes (
           run_id TEXT NOT NULL REFERENCES runtime_runs(id) ON DELETE CASCADE,
           scope_key TEXT NOT NULL REFERENCES context_scopes(scope_key),
           relation TEXT NOT NULL CHECK(relation IN ('shared','parent','focus','current')),
           source TEXT NOT NULL CHECK(source IN ('default','explicit','registry','runtime')),
           epoch INTEGER NOT NULL CHECK(epoch >= 0),
           PRIMARY KEY(run_id,scope_key,relation)
         );
         CREATE INDEX IF NOT EXISTS idx_runtime_run_scopes_scope
           ON runtime_run_scopes(scope_key,run_id);
         CREATE TABLE IF NOT EXISTS conversation_message_scopes (
           message_id TEXT NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
           scope_key TEXT NOT NULL REFERENCES context_scopes(scope_key),
           relation TEXT NOT NULL CHECK(relation IN ('shared','parent','focus','current')),
           PRIMARY KEY(message_id,scope_key,relation)
         );
         CREATE INDEX IF NOT EXISTS idx_conversation_message_scopes_scope
           ON conversation_message_scopes(scope_key,message_id);
         CREATE TRIGGER IF NOT EXISTS context_scope_message_edit
         AFTER UPDATE OF content ON conversation_messages
         WHEN NEW.content != OLD.content
         BEGIN
           UPDATE context_scope_epochs SET epoch=epoch+1
           WHERE scope_key IN (SELECT scope_key FROM conversation_message_scopes WHERE message_id=NEW.id);
         END;
         CREATE TRIGGER IF NOT EXISTS context_scope_message_delete
         BEFORE DELETE ON conversation_messages
         BEGIN
           UPDATE context_scope_epochs SET epoch=epoch+1
           WHERE scope_key IN (SELECT scope_key FROM conversation_message_scopes WHERE message_id=OLD.id);
         END;",
    )?;
    // `CREATE TABLE IF NOT EXISTS` does not evolve installations created by earlier
    // releases. These receipt fields deliberately contain only hashes, never the
    // provider body or context text.
    add_column(
        connection,
        "context_generations",
        "required_set_digest",
        "TEXT CHECK(required_set_digest IS NULL OR length(required_set_digest)=64)",
    )?;
    add_column(
        connection,
        "context_generations",
        "scope_digest",
        "TEXT CHECK(scope_digest IS NULL OR length(scope_digest)=64)",
    )?;
    Ok(())
}

fn add_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name=?1)"),
        [column],
        |row| row.get(0),
    )?;
    if !exists {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))?;
    }
    Ok(())
}
