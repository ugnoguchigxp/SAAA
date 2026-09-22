use rusqlite::{params, Connection, OptionalExtension};

/// The adaptive ledger is optional for legacy databases.  When a receipt did create an adaptive
/// decision, write its terminal technical result in the same database transaction as the routing
/// root so training never sees a completed dispatch without its result.
pub(super) fn record_provider_outcome(
    connection: &Connection,
    run_id: &str,
    technical_success: bool,
    source_id: &str,
    revision: i64,
    now_ms: i64,
) -> Result<(), String> {
    let decision_id = format!("ai-provider-{run_id}");
    let adaptive_schema_present: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ai_decisions')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !adaptive_schema_present {
        return Ok(());
    }
    let present: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ai_decisions WHERE id=?1)",
            [&decision_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(false);
    if present {
        crate::adaptive_improvement::record_outcome_in_transaction(
            connection,
            &decision_id,
            Some(technical_success),
            None,
            None,
            None,
            None,
            None,
            None,
            source_id,
            revision,
            now_ms,
        )?;
    }
    Ok(())
}

/// Event sequences are root-local.  The coordinator may append lifecycle events between receipt
/// and terminal adoption, so completion must never assume a fixed sequence number.
pub(super) fn append_event(
    connection: &Connection,
    root_id: &str,
    kind: &str,
    now_ms: i64,
) -> Result<(), String> {
    let sequence: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM rr_events WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,?2,?3,'{}',?4)",
            params![root_id, sequence, kind, now_ms],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}
