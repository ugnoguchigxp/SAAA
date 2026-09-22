use rusqlite::Connection;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS context_segments (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           previous_segment_id TEXT,
           start_reason TEXT NOT NULL,
           scope_snapshot_json TEXT NOT NULL CHECK(json_valid(scope_snapshot_json)),
           policy_version TEXT NOT NULL,
           bootstrap_tool_schema_digest TEXT NOT NULL,
           renderer_version INTEGER NOT NULL,
           adapter_contract_version INTEGER NOT NULL,
           fixed_render_blob_id TEXT NOT NULL,
           carry_record_id TEXT,
           last_entry_sequence INTEGER NOT NULL DEFAULT 0,
           input_budget INTEGER NOT NULL,
           forget_epoch INTEGER NOT NULL,
           created_at INTEGER NOT NULL,
           status TEXT NOT NULL CHECK(status IN ('active','closed','invalidated'))
         );
         CREATE INDEX IF NOT EXISTS idx_context_segments_active ON context_segments(conversation_id, status, created_at);
         CREATE TABLE IF NOT EXISTS context_entries (
           segment_id TEXT NOT NULL REFERENCES context_segments(id) ON DELETE CASCADE,
           sequence INTEGER NOT NULL CHECK(sequence >= 1),
           role TEXT NOT NULL CHECK(role IN ('carry','user','assistant','tool_round')),
           record_id TEXT,
           rendered_blob_id TEXT NOT NULL,
           serializer_version INTEGER NOT NULL,
           content_digest TEXT NOT NULL,
           dependency_refs_json TEXT NOT NULL CHECK(json_valid(dependency_refs_json)),
           created_at INTEGER NOT NULL,
           PRIMARY KEY(segment_id, sequence)
         );
         CREATE TABLE IF NOT EXISTS context_generation_records (
           generation_id TEXT NOT NULL REFERENCES context_generations(id) ON DELETE CASCADE,
           record_id TEXT NOT NULL,
           usage_kind TEXT NOT NULL CHECK(usage_kind IN ('exposed','claimed','citation','dynamic')),
           range_start INTEGER,
           range_end INTEGER,
           digest TEXT,
           selected INTEGER NOT NULL CHECK(selected IN (0,1)),
           reason TEXT,
           PRIMARY KEY(generation_id, record_id, usage_kind)
         );",
    )?;
    for (table, column, declaration) in [
        ("context_generations", "segment_id", "TEXT"),
        ("context_generations", "entry_range_start", "INTEGER"),
        ("context_generations", "entry_range_end", "INTEGER"),
        ("context_generations", "dynamic_digest", "TEXT"),
        ("context_generations", "wire_digest", "TEXT"),
        ("context_generations", "wire_bytes", "INTEGER"),
        ("context_generations", "omission_json", "TEXT"),
        ("tool_selection_mcp_results", "record_id", "TEXT"),
    ] {
        add_column(connection, table, column, declaration)?;
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
        &format!(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='{table}')"
        ),
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    let column_exists: bool = connection.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name=?1)"),
        [column],
        |row| row.get(0),
    )?;
    if !column_exists {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))?;
    }
    Ok(())
}
