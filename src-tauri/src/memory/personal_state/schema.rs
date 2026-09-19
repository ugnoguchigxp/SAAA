use rusqlite::{params, Connection};

/// Additive v17 migration. Raw text remains in conversation_messages only.
pub fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(include_str!("schema.sql"))?;
    // World DDL and the forget trigger must exist before recover/rebuild runs.
    c.execute_batch(include_str!("world/schema.sql"))?;
    add_column(c, "personal_jobs", "scope_key", "TEXT")?;
    add_column(c, "personal_jobs", "claim_scope_epoch", "INTEGER")?;
    add_column(c, "personal_generations", "context_generation_id", "TEXT")?;
    let created = c.execute(
        "INSERT OR IGNORE INTO personal_scope(id,principal) VALUES('primary',?1)",
        [crate::new_id("principal")],
    )?;
    if created == 1 {
        c.execute(
            "UPDATE working_state_items SET status='expired' WHERE status='active'",
            [],
        )?;
        c.execute(
            "UPDATE continuity_capsule_items SET status='stale' WHERE status='active'",
            [],
        )?;
        c.execute(
            "UPDATE continuity_capsule_revisions SET status='failed' WHERE status='active'",
            [],
        )?;
    }
    // The initial import is metadata-only; none of this history is marked processed.
    c.execute("INSERT OR IGNORE INTO personal_sources(message_id,version,role,bytes,recorded_at) SELECT m.id,1,m.role,length(CAST(m.content AS BLOB)),CAST(m.created_at AS INTEGER) FROM conversation_messages m WHERE m.conversation_id=?1 AND m.role IN ('user','assistant','transcript') ORDER BY m.rowid", [crate::PRIMARY_CONVERSATION_ID])?;
    c.execute("INSERT OR IGNORE INTO personal_jobs(source_sequence,epoch,status) SELECT sequence,(SELECT input_epoch FROM personal_scope WHERE id='primary'),'queued' FROM personal_sources WHERE available=1", [])?;
    c.execute("UPDATE personal_jobs SET status='queued',lease_until=NULL,lease_generation=lease_generation+1,result_code='lease-expired' WHERE status='running' AND lease_until<=?1", params![crate::now_iso().parse::<i64>().unwrap_or(0)])?;
    c.execute("UPDATE personal_registrations SET pins=0,desired='deleted' WHERE incarnation IN (SELECT incarnation FROM personal_cleanup WHERE stage!='complete')", [])?;
    // A ready view must never survive process restart as a reusable capability.
    c.execute("UPDATE personal_generations SET output_allowed=0,status='interrupted' WHERE status IN ('prepared','running')", [])?;
    let mut statement = c.prepare("SELECT source_id,forgotten_at FROM personal_tombstones")?;
    let tombstones = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    super::store::recover(c, Some(&tombstones)).map_err(rusqlite::Error::InvalidParameterName)?;
    c.execute_batch(include_str!("product_schema.sql"))?;
    Ok(())
}

fn add_column(
    c: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> rusqlite::Result<()> {
    let exists: bool = c.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name=?1)"),
        [column],
        |row| row.get(0),
    )?;
    if !exists {
        c.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))?;
    }
    Ok(())
}
