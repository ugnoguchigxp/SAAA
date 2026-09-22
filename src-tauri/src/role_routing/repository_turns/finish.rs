use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use super::ledger::{append_event, record_provider_outcome};

pub(crate) fn record_provider_turn_finish(
    connection: &Connection,
    run_id: &str,
    status: &str,
    message_id: Option<&str>,
    now_ms: i64,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let root: Option<(String, i64, i64)> = transaction
        .query_row(
            "SELECT phase,cancel_requested,revision FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((current_phase, cancel_requested, root_revision)) = root else {
        return Ok(());
    };
    // `accept_provider_turn` runs inside the assistant-message transaction first.  The outer
    // runtime finalizer still observes the provider terminal result afterwards, but must not
    // append a second terminal event or overwrite the already adopted root.
    if matches!(current_phase.as_str(), "completed" | "cancelled" | "failed") {
        return Ok(());
    }
    // A cancelled or barrier-held root is owned by the cancel / barrier path. The outer
    // finalizer must not turn a late provider result into the current answer.
    if cancel_requested != 0 {
        return Ok(());
    }
    if current_phase == "draining" {
        let step = active_step_or_ordinal_zero(&transaction, run_id)?;
        if let Some((step_id, _)) = step.as_ref() {
            let step_status = match status {
                "completed" => "succeeded",
                "cancelled" => "cancelled",
                _ => "failed",
            };
            crate::role_routing::steps::complete_step(
                &transaction,
                run_id,
                step_id,
                step_status,
                now_ms,
            )?;
        }
        return transaction.commit().map_err(|error| error.to_string());
    }
    if current_phase != "responding" {
        return Err("Role-routing root has an invalid phase at provider completion".into());
    }
    let step_status = match status {
        "completed" => "succeeded",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    let terminal_phase = match status {
        "completed" => "completed",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    let Some((step_id, step_revision)) = active_step_or_ordinal_zero(&transaction, run_id)? else {
        return Err("Role-routing root has no step to complete".into());
    };
    if step_revision != root_revision {
        return Err("Role-routing provider result belongs to a stale revision".into());
    }
    if !crate::role_routing::steps::complete_step(
        &transaction,
        run_id,
        &step_id,
        step_status,
        now_ms,
    )? {
        return Err("Role-routing provider step was already completed".into());
    }
    if status != "completed" {
        let remaining_status = if status == "cancelled" {
            "cancelled"
        } else {
            "interrupted"
        };
        crate::role_routing::steps::settle_unfinished_steps(
            &transaction,
            run_id,
            root_revision,
            remaining_status,
            now_ms,
        )?;
    }
    if status == "completed" {
        let message_id =
            message_id.ok_or_else(|| "Completed role-routing result has no message".to_string())?;
        crate::role_routing::steps::record_step_output(
            &transaction,
            run_id,
            &step_id,
            step_revision,
            "answer",
            &json!({ "messageId": message_id }).to_string(),
            true,
            now_ms,
        )?;
    }
    let changed = transaction
        .execute(
            "UPDATE rr_roots SET phase=?1, result_message_id=?2, active_slot=NULL WHERE root_id=?3 AND phase='responding' AND revision=?4 AND cancel_requested=0",
            params![terminal_phase, message_id, run_id, root_revision],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing root changed before provider completion committed".into());
    }
    append_event(
        &transaction,
        run_id,
        if status == "completed" {
            "answer_committed"
        } else {
            "root_finished"
        },
        now_ms,
    )?;
    record_provider_outcome(
        &transaction,
        run_id,
        status == "completed",
        message_id.unwrap_or(run_id),
        step_revision,
        now_ms,
    )?;
    crate::role_routing::learning::repository::mark_root_dirty(&transaction, run_id)?;
    transaction.commit().map_err(|error| error.to_string())
}

/// Prefers the active step, falling back to ordinal 0 for legacy rows created before the step
/// ledger selected the active step.
fn active_step_or_ordinal_zero(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<(String, i64)>, String> {
    let running = connection
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status IN ('running','draining') ORDER BY ordinal LIMIT 1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match running {
        Some(active) => Ok(Some(active)),
        None => connection
            .query_row(
                "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND ordinal=0",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string()),
    }
}
