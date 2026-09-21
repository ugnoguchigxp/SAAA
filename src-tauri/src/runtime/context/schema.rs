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
    backfill_legacy_coding_scopes(connection)?;
    Ok(())
}

fn backfill_legacy_coding_scopes(connection: &Connection) -> rusqlite::Result<()> {
    let has_workspaces: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='coding_workspaces')",
        [],
        |row| row.get(0),
    )?;
    if !has_workspaces {
        return Ok(());
    }
    connection.execute_batch(
        "INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
         SELECT 'project:' || id,'project',id,'active',strftime('%s','now') || '000'
         FROM coding_workspaces
         WHERE true
         ON CONFLICT(scope_key) DO UPDATE SET state='active';
         INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
         SELECT 'resource:' || id,'resource',id,'active',strftime('%s','now') || '000'
         FROM coding_workspaces
         WHERE true
         ON CONFLICT(scope_key) DO UPDATE SET state='active';
         INSERT OR IGNORE INTO context_scope_epochs(scope_key,epoch)
         SELECT scope_key,0 FROM context_scopes
         WHERE kind IN ('project','resource');
         INSERT OR IGNORE INTO context_scope_links(
           parent_scope_key,child_scope_key,relation,created_at
         )
         SELECT 'project:' || id,'resource:' || id,'parent',strftime('%s','now') || '000'
         FROM coding_workspaces;",
    )?;

    let has_jobs: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='coding_jobs')",
        [],
        |row| row.get(0),
    )?;
    if has_jobs {
        connection.execute_batch(
            "INSERT OR IGNORE INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
             SELECT 'task:' || jobs.id,'task',jobs.id,'active',strftime('%s','now') || '000'
             FROM coding_jobs AS jobs
             JOIN coding_workspaces AS workspaces ON workspaces.id=jobs.workspace_id
             WHERE jobs.state IN ('queued','running','cancel_requested');
             INSERT OR IGNORE INTO context_scope_epochs(scope_key,epoch)
             SELECT scope_key,0 FROM context_scopes WHERE kind='task';
             INSERT OR IGNORE INTO context_scope_links(
               parent_scope_key,child_scope_key,relation,created_at
             )
             SELECT 'resource:' || jobs.workspace_id,'task:' || jobs.id,'parent',
                    strftime('%s','now') || '000'
             FROM coding_jobs AS jobs
             JOIN coding_workspaces AS workspaces ON workspaces.id=jobs.workspace_id
             WHERE jobs.state IN ('queued','running','cancel_requested');",
        )?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additive_migration_preserves_old_generations_and_adds_digest_receipts() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE provider_sessions(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY, content TEXT);
                 CREATE TABLE context_generations (
                   id TEXT PRIMARY KEY,
                   run_id TEXT NOT NULL,
                   provider_session_id TEXT,
                   provider_id TEXT NOT NULL,
                   ordinal INTEGER NOT NULL,
                   purpose TEXT NOT NULL,
                   envelope_digest TEXT NOT NULL,
                   request_digest TEXT NOT NULL,
                   projected_bytes INTEGER NOT NULL,
                   current_instruction_count INTEGER NOT NULL,
                   health_status TEXT NOT NULL,
                   status TEXT NOT NULL,
                   failure_kind TEXT,
                   started_at TEXT NOT NULL,
                   completed_at TEXT,
                   UNIQUE(run_id, ordinal)
                 );
                 INSERT INTO runtime_runs VALUES('run');
                 INSERT INTO context_generations VALUES(
                   'generation','run',NULL,'provider',1,'reasoning',
                   'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                   'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                   42,1,'green','completed',NULL,'1','1'
                 );",
            )
            .unwrap();

        migrate(&connection).unwrap();
        migrate(&connection).unwrap();

        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('context_generations') ORDER BY cid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(columns.contains(&"required_set_digest".to_string()));
        assert!(columns.contains(&"scope_digest".to_string()));
        let receipt: (Option<String>, Option<String>, String) = connection
            .query_row(
                "SELECT required_set_digest,scope_digest,request_digest
                 FROM context_generations WHERE id='generation'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(receipt.0, None);
        assert_eq!(receipt.1, None);
        assert_eq!(receipt.2.len(), 64);
    }

    #[test]
    fn migration_registers_scopes_for_legacy_coding_workspaces() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE provider_sessions(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY, content TEXT);
                 CREATE TABLE coding_workspaces(
                   id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL,path TEXT NOT NULL
                 );
                 INSERT INTO coding_workspaces VALUES('workspace_legacy','conversation','/tmp/project');",
            )
            .unwrap();

        migrate(&connection).unwrap();
        migrate(&connection).unwrap();

        let scopes: Vec<(String, String, String)> = connection
            .prepare(
                "SELECT scope_key,kind,state FROM context_scopes
                 WHERE opaque_id='workspace_legacy' ORDER BY scope_key",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            scopes,
            vec![
                (
                    "project:workspace_legacy".into(),
                    "project".into(),
                    "active".into()
                ),
                (
                    "resource:workspace_legacy".into(),
                    "resource".into(),
                    "active".into()
                )
            ]
        );
        let linked: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM context_scope_links
                 WHERE parent_scope_key='project:workspace_legacy'
                   AND child_scope_key='resource:workspace_legacy')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(linked);
    }
}
