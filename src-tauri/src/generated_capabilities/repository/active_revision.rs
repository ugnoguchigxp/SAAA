use super::*;
pub fn finish_import(
    connection: &Connection,
    import_id: &str,
    status: &str,
    revision_id: Option<&str>,
    error_code: Option<&str>,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_imports
             SET status = ?2, revision_id = ?3, error_code = ?4, completed_at = ?5
             WHERE id = ?1",
            params![import_id, status, revision_id, error_code, now],
        )
        .map_err(storage)?;
    Ok(())
}
pub fn insert_check(
    connection: &Connection,
    check_id: &str,
    revision_id: &str,
    runtime_digest: &str,
    inventory_hash: &str,
    acceptance_hash: &str,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_checks(
               id, revision_id, runtime_digest, inventory_hash, acceptance_hash, status,
               error_code, report_ref, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', NULL, NULL, ?6, NULL)",
            params![
                check_id,
                revision_id,
                runtime_digest,
                inventory_hash,
                acceptance_hash,
                now
            ],
        )
        .map_err(storage)?;
    Ok(())
}
pub fn finish_check(
    connection: &Connection,
    check_id: &str,
    status: &str,
    error_code: Option<&str>,
    report_ref: Option<&str>,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_checks
             SET status = ?2, error_code = ?3, report_ref = ?4, completed_at = ?5
             WHERE id = ?1",
            params![check_id, status, error_code, report_ref, now],
        )
        .map_err(storage)?;
    Ok(())
}
pub fn running_check_exists(connection: &Connection, revision_id: &str) -> CapabilityResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM generated_capability_checks WHERE revision_id = ?1 AND status = 'running')",
            params![revision_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage)
}
pub fn insert_call(
    connection: &Connection,
    call_id: &str,
    revision_id: &str,
    package_hash: &str,
    origin: &str,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_calls(
               id, revision_id, package_hash, origin, status, result_bool, error_code, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, 'running', NULL, NULL, ?5, NULL)",
            params![call_id, revision_id, package_hash, origin, now],
        )
        .map_err(storage)?;
    Ok(())
}
pub fn finish_call(
    connection: &Connection,
    call_id: &str,
    status: &str,
    result: Option<bool>,
    error_code: Option<&str>,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_calls
             SET status = ?2, result_bool = ?3, error_code = ?4, completed_at = ?5
             WHERE id = ?1",
            params![call_id, status, result, error_code, now],
        )
        .map_err(storage)?;
    Ok(())
}
/// Moves one check to `interrupted` only if it is still `running`.
pub fn interrupt_check(
    connection: &Connection,
    check_id: &str,
    now: &str,
) -> CapabilityResult<usize> {
    connection
        .execute(
            "UPDATE generated_capability_checks SET status = 'interrupted', completed_at = ?2
             WHERE id = ?1 AND status = 'running'",
            params![check_id, now],
        )
        .map_err(storage)
}
/// Moves one import to `interrupted` only if it is still `staging`.
pub fn interrupt_import(
    connection: &Connection,
    import_id: &str,
    now: &str,
) -> CapabilityResult<usize> {
    connection
        .execute(
            "UPDATE generated_capability_imports SET status = 'interrupted', completed_at = ?2
             WHERE id = ?1 AND status = 'staging'",
            params![import_id, now],
        )
        .map_err(storage)
}
/// Counts records that never reached a terminal status.
pub fn unsettled_rows(connection: &Connection) -> CapabilityResult<i64> {
    connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'running')
                  + (SELECT COUNT(*) FROM generated_capability_checks WHERE status = 'running')
                  + (SELECT COUNT(*) FROM generated_capability_imports WHERE status = 'staging')",
            [],
            |row| row.get(0),
        )
        .map_err(storage)
}
/// Recovery: any check/call/import left running belongs to a previous process.
pub fn interrupt_running(connection: &Connection) -> CapabilityResult<(usize, usize, usize)> {
    let now = crate::now_iso();
    let checks = connection
        .execute(
            "UPDATE generated_capability_checks SET status = 'interrupted', completed_at = ?1
             WHERE status = 'running'",
            params![now],
        )
        .map_err(storage)?;
    let calls = connection
        .execute(
            "UPDATE generated_capability_calls SET status = 'interrupted', completed_at = ?1
             WHERE status = 'running'",
            params![now],
        )
        .map_err(storage)?;
    let imports = connection
        .execute(
            "UPDATE generated_capability_imports SET status = 'interrupted', completed_at = ?1
             WHERE status = 'staging'",
            params![now],
        )
        .map_err(storage)?;
    Ok((checks, calls, imports))
}
/// Recovery: an active revision whose pointer disagrees, or whose package is gone, is stopped.
pub fn stop_inconsistent_capabilities(
    connection: &Connection,
    now: &str,
) -> CapabilityResult<Vec<String>> {
    let mut inconsistent: Vec<String> = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT c.id FROM generated_capabilities c
                 LEFT JOIN generated_capability_revisions r
                   ON r.id = c.current_revision_id AND r.capability_id = c.id
                 WHERE c.current_revision_id IS NOT NULL
                   AND (r.id IS NULL OR r.state <> 'active')",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage)?;
        for row in rows {
            inconsistent.push(row.map_err(storage)?);
        }
    }
    {
        let mut statement = connection
            .prepare(
                "SELECT capability_id FROM generated_capability_revisions WHERE state = 'active'",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage)?;
        for row in rows {
            let capability_id = row.map_err(storage)?;
            let capability = capability_by_id(connection, &capability_id)?;
            let matched = capability
                .current_revision_id
                .as_deref()
                .map(|id| {
                    connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM generated_capability_revisions
                              WHERE id = ?1 AND capability_id = ?2 AND state = 'active')",
                            params![id, capability_id],
                            |row| row.get::<_, bool>(0),
                        )
                        .map_err(storage)
                })
                .transpose()?
                .unwrap_or(false);
            if !matched {
                inconsistent.push(capability_id);
            }
        }
    }
    inconsistent.sort();
    inconsistent.dedup();
    for capability_id in &inconsistent {
        connection
            .execute(
                "UPDATE generated_capability_revisions SET state = 'suspended'
                 WHERE capability_id = ?1 AND state = 'active'",
                params![capability_id],
            )
            .map_err(storage)?;
        connection
            .execute(
                "UPDATE generated_capabilities
                 SET current_revision_id = NULL, catalog_epoch = catalog_epoch + 1, updated_at = ?2
                 WHERE id = ?1",
                params![capability_id, now],
            )
            .map_err(storage)?;
    }
    Ok(inconsistent)
}
pub fn package_hashes_in_use(connection: &Connection) -> CapabilityResult<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT DISTINCT package_hash FROM generated_capability_revisions")
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage)?;
    let mut hashes = Vec::new();
    for row in rows {
        hashes.push(row.map_err(storage)?);
    }
    Ok(hashes)
}
#[derive(Clone, Debug)]
pub struct ActiveRevision {
    pub revision_id: String,
    pub capability_id: String,
    pub package_hash: String,
    pub inventory_hash: String,
}
pub fn active_revisions(connection: &Connection) -> CapabilityResult<Vec<ActiveRevision>> {
    let mut statement = connection
        .prepare(
            "SELECT id, capability_id, package_hash, inventory_hash
             FROM generated_capability_revisions WHERE state = 'active'",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| {
            Ok(ActiveRevision {
                revision_id: row.get(0)?,
                capability_id: row.get(1)?,
                package_hash: row.get(2)?,
                inventory_hash: row.get(3)?,
            })
        })
        .map_err(storage)?;
    let mut revisions = Vec::new();
    for row in rows {
        revisions.push(row.map_err(storage)?);
    }
    Ok(revisions)
}
