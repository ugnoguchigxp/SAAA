use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use super::ledger::{append_event, record_provider_outcome};

/// Cancels every queued or active routing root when the feature is disabled. The database fence
/// commits with the settings change; process-local cancellation is signalled by the command layer
/// after this transaction returns.
pub(crate) fn cancel_all_for_disable(connection: &Connection, now_ms: i64) -> Result<(), String> {
    let root_ids = connection
        .prepare(
            "SELECT root_id FROM rr_roots
             WHERE phase IN ('queued','responding','draining')
             ORDER BY started_at_ms,root_id",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for root_id in root_ids {
        crate::role_routing::coordinator::apply_in_transaction(
            connection,
            &root_id,
            crate::role_routing::reducer::Event::Cancel,
            now_ms,
        )?;
        connection
            .execute(
                "UPDATE rr_steps SET status='cancelled',cancel_requested=1,completed_at_ms=?1,error_code='routing-disabled'
                 WHERE root_id=?2 AND status IN ('planned','running','draining')",
                params![now_ms, root_id],
            )
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute(
            "UPDATE rr_speech SET status='cancelled',updated_at_ms=?1
             WHERE status IN ('queued','playing')",
            [now_ms],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE rr_premium_proposals SET status='expired'
             WHERE status IN ('proposed','approved') AND consumed_at_ms IS NULL",
            [],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// While routing is disabled, legacy execution stays fenced until every cancelled runtime child
/// and role-owned tool operation is terminal. This avoids overlapping old side effects with a new
/// legacy answer.
pub(crate) fn disable_drain_in_progress(connection: &Connection) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM rr_roots r
               JOIN runtime_runs run ON run.id=r.runtime_run_id
               WHERE r.cancel_requested=1 AND run.status IN ('running','queued')
               UNION ALL
               SELECT 1 FROM rr_tool_links l
               JOIN rr_roots r ON r.root_id=l.root_id
               WHERE r.cancel_requested=1 AND l.dispatch_state IN ('reserved','dispatched')
               UNION ALL
               SELECT 1 FROM rr_speech s
               JOIN rr_roots r ON r.root_id=s.root_id
               WHERE r.cancel_requested=1 AND s.status IN ('queued','playing')
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

pub(crate) fn disabled_runtime_run_ids(connection: &Connection) -> Result<Vec<String>, String> {
    connection
        .prepare(
            "SELECT DISTINCT runtime_run_id FROM rr_roots
             WHERE cancel_requested=1 AND runtime_run_id IS NOT NULL",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Stores coarse actor progress without retaining any partial provider text.
pub(crate) fn record_actor_activity(
    connection: &Connection,
    root_id: &str,
    kind: &str,
    now_ms: i64,
) -> Result<bool, String> {
    if !matches!(kind, "provider_started" | "provider_progress") {
        return Err("Unsupported role-routing activity kind".into());
    }
    let active = connection
        .query_row(
            "SELECT phase IN ('queued','responding','draining') FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(false);
    if !active {
        return Ok(false);
    }
    append_event(connection, root_id, "activity", now_ms)?;
    Ok(true)
}

/// Atomically adopts a normal provider result when the role-routing root is still current.
/// It is invoked from the assistant-message transaction after the message and scope link exist.
pub(crate) fn accept_provider_turn(
    connection: &Connection,
    run_id: &str,
    message_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    accept_provider_turn_with_status(connection, run_id, message_id, now_ms, "succeeded")
}

pub(crate) fn accept_provider_turn_with_status(
    connection: &Connection,
    run_id: &str,
    message_id: &str,
    now_ms: i64,
    step_status: &str,
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
    // Only the active `responding` phase may adopt a final answer. `draining` means an input
    // barrier or a stop request is up, and terminal phases are already settled; a result that
    // arrives then is held or dropped by the caller, never recorded as the current answer.
    if phase != "responding" {
        return Err(format!(
            "Role-routing result is not adoptable from phase {phase}"
        ));
    }
    // The expected revision comes from the result side step row, not from the root's current
    // revision. A late result produced for an older revision must not be adopted against the
    // current one even if the root has since advanced. The active step is selected by the step
    // ledger, so ordinal 0 is not hard-coded.
    let Some((step_id, step_revision)) =
        crate::role_routing::steps::active_reasoning_step(connection, run_id)?
    else {
        return Err("Role-routing result has no active step".into());
    };
    if step_revision != revision {
        return Err("Role-routing result is stale for the active step".into());
    }
    if !crate::role_routing::steps::complete_step(
        connection,
        run_id,
        &step_id,
        step_status,
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

/// Stores SDK-reported usage in the same transaction that adopts the final answer. The value is
/// produced by the typed sidecar protocol and checked again by SQLite's JSON constraint.
pub(crate) fn record_step_usage(
    connection: &Connection,
    run_id: &str,
    usage_json: &str,
) -> Result<(), String> {
    let Some((step_id, _revision)) =
        crate::role_routing::steps::active_reasoning_step(connection, run_id)?
    else {
        return Err("Role-routing usage is stale or has no active step".into());
    };
    let changed = connection
        .execute(
            "UPDATE rr_steps SET usage_json=?1 WHERE id=?2 AND root_id=?3 AND status IN ('planned','running','draining')",
            params![usage_json, step_id, run_id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing usage is stale or has no active step".into());
    }
    Ok(())
}
