use super::*;

/// Finishes a turn at the receptionist. Later planned steps, including the reasoner, are
/// cancelled in the same transaction so recovery cannot start them.
pub(crate) fn accept_resolved_frontend(
    connection: &Connection,
    run_id: &str,
    message_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let root: Option<(i64, i64, String)> = connection
        .query_row(
            "SELECT revision,cancel_requested,phase FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((revision, cancelled, phase)) = root else {
        return Ok(());
    };
    if cancelled != 0 {
        return Err("Role-routing result was cancelled".into());
    }
    if phase != "responding" {
        return Err(format!(
            "Role-routing result is not adoptable from phase {phase}"
        ));
    }
    let Some((step_id, step_revision)) = connection
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' AND purpose='frontend' ORDER BY ordinal LIMIT 1",
            [run_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?
    else {
        return Err("Role-routing result has no active frontend step".into());
    };
    if step_revision != revision {
        return Err("Role-routing result is stale for the active step".into());
    }
    connection
        .execute(
            "UPDATE rr_steps SET status='cancelled',completed_at_ms=?1 WHERE root_id=?2 AND revision=?3 AND status='planned'",
            params![now_ms, run_id, revision],
        )
        .map_err(|error| error.to_string())?;
    if !crate::role_routing::steps::complete_step(
        connection,
        run_id,
        &step_id,
        "succeeded",
        now_ms,
    )? {
        return Err("Role-routing result step already completed".into());
    }
    if !crate::role_routing::steps::finalize_root(connection, run_id, revision, message_id, now_ms)?
    {
        return Err("Role-routing result is stale or already accepted".into());
    }
    crate::role_routing::steps::record_step_output(
        connection,
        run_id,
        &step_id,
        revision,
        "answer",
        &json!({ "messageId": message_id }).to_string(),
        true,
        now_ms,
    )?;
    append_event(connection, run_id, "answer_committed", now_ms)?;
    record_provider_outcome(connection, run_id, true, message_id, revision, now_ms)?;
    crate::role_routing::learning::repository::mark_root_dirty(connection, run_id)?;
    Ok(())
}
