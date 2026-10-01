use rusqlite::Connection;
pub(super) fn add_column(
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

pub(super) fn initialize(c: &Connection) -> rusqlite::Result<()> {
    add_column(
        c,
        "personal_scope",
        "world_turn",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(
        c,
        "personal_maintenance",
        "enabled",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    // World DDL and the forget trigger must exist before recover/rebuild runs.
    c.execute_batch(include_str!("world/schema.sql"))?;
    add_column(
        c,
        "personal_world_projection_meta",
        "projection_version",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column(
        c,
        "personal_jobs",
        "retry_count",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(
        c,
        "personal_jobs",
        "stage",
        "TEXT NOT NULL DEFAULT 'continuity'",
    )?;
    add_column(c, "personal_jobs", "scope_key", "TEXT")?;
    add_column(c, "personal_jobs", "claim_scope_epoch", "INTEGER")?;
    add_column(c, "personal_generations", "context_generation_id", "TEXT")?;
    Ok(())
}
