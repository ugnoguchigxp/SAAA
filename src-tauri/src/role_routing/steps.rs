//! Step-level ledger operations for the role-routing root.
//!
//! A root owns at most one active reasoning step (enforced by
//! `idx_rr_steps_one_active_reasoning`). The functions here make step claims and terminal
//! transitions explicit and idempotent so a coordinator restart cannot turn every planned step
//! into `running` or record the same completion twice.

use rusqlite::{params, Connection, OptionalExtension};

/// Terminal step statuses. Repeating the same terminal status is a no-op; a conflicting status
/// is rejected so a late or duplicated completion cannot rewrite settled history.
pub(crate) const TERMINAL_STEP_STATUSES: [&str; 4] =
    ["succeeded", "failed", "cancelled", "interrupted"];

fn is_terminal(status: &str) -> bool {
    TERMINAL_STEP_STATUSES.contains(&status)
}

/// Claims exactly one planned step for the root/revision. The lowest ordinal wins, so a recipe
/// with several planned steps cannot start them all at once. Returns `None` when no planned step
/// is available (for example after a resume whose next step has not been created yet).
pub(crate) fn claim_next_planned_step(
    transaction: &Connection,
    root_id: &str,
    revision: i64,
    now_ms: i64,
) -> Result<Option<String>, String> {
    let step_id: Option<String> = transaction
        .query_row(
            "SELECT id FROM rr_steps WHERE root_id=?1 AND revision=?2 AND status='planned' ORDER BY ordinal LIMIT 1",
            params![root_id, revision],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(step_id) = step_id else {
        return Ok(None);
    };
    let changed = transaction
        .execute(
            "UPDATE rr_steps SET status='running',started_at_ms=COALESCE(started_at_ms,?1) WHERE id=?2 AND root_id=?3 AND status='planned'",
            params![now_ms, step_id, root_id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing planned step changed before it could be claimed".into());
    }
    Ok(Some(step_id))
}

/// Records a terminal step status at most once. Returns `true` when the transition happened and
/// `false` when the step was already in the same terminal status. A different terminal status on
/// an already-terminal step is a conflict, not a silent overwrite.
pub(crate) fn complete_step(
    transaction: &Connection,
    root_id: &str,
    step_id: &str,
    status: &str,
    now_ms: i64,
) -> Result<bool, String> {
    if !is_terminal(status) {
        return Err("Role-routing step completion requires a terminal status".into());
    }
    let current: Option<String> = transaction
        .query_row(
            "SELECT status FROM rr_steps WHERE id=?1 AND root_id=?2",
            params![step_id, root_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(current) = current else {
        return Err("Role-routing step was not found".into());
    };
    if is_terminal(&current) {
        return if current == status {
            Ok(false)
        } else {
            Err("Role-routing step already has a different terminal status".into())
        };
    }
    let changed = transaction
        .execute(
            "UPDATE rr_steps SET status=?1,completed_at_ms=?2 WHERE id=?3 AND root_id=?4 AND status IN ('planned','running','draining')",
            params![status, now_ms, step_id, root_id],
        )
        .map_err(|error| error.to_string())?;
    if changed == 1 {
        return Ok(true);
    }
    // Never report an unconfirmed zero-row mutation as a successful no-op. Re-read the row so
    // an exact duplicate remains idempotent while every conflicting transition fails closed.
    let settled: Option<String> = transaction
        .query_row(
            "SELECT status FROM rr_steps WHERE id=?1 AND root_id=?2",
            params![step_id, root_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match settled.as_deref() {
        Some(current) if current == status => Ok(false),
        Some(_) => Err("Role-routing step changed during completion".into()),
        None => Err("Role-routing step disappeared during completion".into()),
    }
}

/// Settles every unfinished step in the current revision after the root reaches a terminal
/// failure/cancellation state. A failed root must not retain `planned` work that recovery could
/// later mistake for resumable work.
pub(crate) fn settle_unfinished_steps(
    transaction: &Connection,
    root_id: &str,
    revision: i64,
    status: &str,
    now_ms: i64,
) -> Result<usize, String> {
    if !matches!(status, "cancelled" | "interrupted") {
        return Err("Unfinished role-routing steps require a non-success terminal status".into());
    }
    transaction
        .execute(
            "UPDATE rr_steps SET status=?1,completed_at_ms=?2 WHERE root_id=?3 AND revision=?4 AND status IN ('planned','running','draining')",
            params![status, now_ms, root_id, revision],
        )
        .map_err(|error| error.to_string())
}

/// Returns the currently active reasoning step for a root, if any. This replaces the ordinal-0
/// assumption: the step ledger decides which step a provider or tool result belongs to.
pub(crate) fn active_reasoning_step(
    connection: &Connection,
    root_id: &str,
) -> Result<Option<(String, i64)>, String> {
    connection
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' AND purpose IN ('respond','reconsider','revise') ORDER BY ordinal LIMIT 1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())
}

/// Records a step output. Intermediate draft/review outputs use `accepted = false` and never
/// touch the root terminal state; only a final adoption records `accepted = true`. The output ID
/// is derived from the step so a duplicate terminal result collides instead of silently
/// overwriting history.
pub(crate) fn record_step_output(
    connection: &Connection,
    root_id: &str,
    step_id: &str,
    revision: i64,
    kind: &str,
    payload_json: &str,
    accepted: bool,
    now_ms: i64,
) -> Result<String, String> {
    let step_revision: Option<i64> = connection
        .query_row(
            "SELECT revision FROM rr_steps WHERE id=?1 AND root_id=?2",
            params![step_id, root_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(step_revision) = step_revision else {
        return Err("Role-routing output references an unknown step".into());
    };
    if step_revision != revision {
        return Err("Role-routing output revision does not match its step".into());
    }
    let output_id = format!("rr-output-{step_id}");
    connection
        .execute(
            "INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                output_id,
                step_id,
                revision,
                kind,
                payload_json,
                i64::from(accepted),
                now_ms
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(output_id)
}

/// Adopts the final answer for a root exactly once. Returns `true` when the root transitions to
/// `completed`, `false` for an idempotent repeat with the same message, and an error when the
/// root moved on (new revision, cancellation, or a different accepted message).
pub(crate) fn finalize_root(
    connection: &Connection,
    root_id: &str,
    revision: i64,
    message_id: &str,
    now_ms: i64,
) -> Result<bool, String> {
    let changed = connection
        .execute(
            "UPDATE rr_roots SET phase='completed',result_message_id=?1,active_slot=NULL WHERE root_id=?2 AND phase='responding' AND revision=?3 AND cancel_requested=0",
            params![message_id, root_id, revision],
        )
        .map_err(|error| error.to_string())?;
    if changed == 1 {
        return Ok(true);
    }
    let existing: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT phase,result_message_id FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match existing {
        Some((phase, Some(existing_message)))
            if phase == "completed" && existing_message == message_id =>
        {
            let _ = now_ms;
            Ok(false)
        }
        _ => Err("Role-routing finalization conflicts with the current root state".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(id TEXT PRIMARY KEY);
                 CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);
                 INSERT INTO conversations VALUES('c');
                 INSERT INTO conversation_messages VALUES('message-a');
                 INSERT INTO conversation_messages VALUES('message-b');
                 INSERT INTO conversation_messages VALUES('message-c');",
            )
            .expect("base tables");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection
            .execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','responding','text','visual',1,'')", [])
            .expect("root");
        connection
    }

    fn planned_step(connection: &Connection, id: &str, ordinal: i64) {
        connection
            .execute(
                "INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES(?1,'r',0,?2,'actor','respond','planned','{}','{}')",
                params![id, ordinal],
            )
            .expect("planned step");
    }

    #[test]
    fn rr_02_one_active_reasoning_step() {
        let connection = fixture();
        planned_step(&connection, "s1", 0);
        planned_step(&connection, "s2", 1);
        // Planned steps may coexist; the unique index only forbids two active reasoning steps.
        assert!(connection
            .execute("UPDATE rr_steps SET status='running' WHERE id='s1'", [])
            .is_ok());
        assert!(
            connection
                .execute("UPDATE rr_steps SET status='running' WHERE id='s2'", [])
                .is_err(),
            "a second running reasoning step must be rejected"
        );
        // A frontend step uses the separate slot and may run alongside the reasoner.
        connection
            .execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('front','r',0,2,'actor','frontend','running','{}','{}')", [])
            .expect("frontend step");
    }

    #[test]
    fn rr_05_duplicate_completion_once() {
        let mut connection = fixture();
        planned_step(&connection, "s1", 0);
        let transaction = connection.transaction().expect("tx");
        assert_eq!(
            claim_next_planned_step(&transaction, "r", 0, 2).expect("claim"),
            Some("s1".into())
        );
        assert_eq!(
            claim_next_planned_step(&transaction, "r", 0, 3).expect("claim"),
            None
        );
        assert!(complete_step(&transaction, "r", "s1", "succeeded", 4).expect("complete"));
        assert!(!complete_step(&transaction, "r", "s1", "succeeded", 5).expect("idempotent"));
        assert!(complete_step(&transaction, "r", "s1", "failed", 6).is_err());
        transaction.commit().expect("commit");
        let status: String = connection
            .query_row("SELECT status FROM rr_steps WHERE id='s1'", [], |row| {
                row.get(0)
            })
            .expect("status");
        assert_eq!(status, "succeeded");
    }

    #[test]
    fn rr_12_intermediate_output_not_final() {
        let connection = fixture();
        planned_step(&connection, "draft", 0);
        planned_step(&connection, "review", 1);
        // Draft and review outputs are recorded as unaccepted and do not complete the root.
        record_step_output(&connection, "r", "draft", 0, "draft", "{}", false, 2)
            .expect("draft output");
        assert!(complete_step(&connection, "r", "draft", "succeeded", 3).expect("draft"));
        assert_eq!(
            connection
                .query_row("SELECT phase FROM rr_roots WHERE root_id='r'", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("phase"),
            "responding"
        );
        record_step_output(&connection, "r", "review", 0, "review", "{}", false, 4)
            .expect("review output");
        assert!(complete_step(&connection, "r", "review", "succeeded", 5).expect("review"));
        let unaccepted: i64 = connection
            .query_row(
                "SELECT count(*) FROM rr_outputs WHERE accepted=0",
                [],
                |row| row.get(0),
            )
            .expect("unaccepted");
        assert_eq!(unaccepted, 2);
        assert_eq!(
            connection
                .query_row("SELECT phase FROM rr_roots WHERE root_id='r'", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("phase"),
            "responding"
        );
        // Only the final adoption moves the root to completed.
        assert!(finalize_root(&connection, "r", 0, "message-a", 6).expect("finalize"));
        assert_eq!(
            connection
                .query_row("SELECT phase FROM rr_roots WHERE root_id='r'", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("phase"),
            "completed"
        );
    }

    #[test]
    fn rr_12_finalize_once() {
        let connection = fixture();
        planned_step(&connection, "s1", 0);
        assert!(finalize_root(&connection, "r", 0, "message-a", 2).expect("first finalize"));
        assert!(!finalize_root(&connection, "r", 0, "message-a", 3).expect("idempotent"));
        assert!(finalize_root(&connection, "r", 0, "message-b", 4).is_err());
        // The same accepted message stays idempotent even if the caller repeats it.
        assert!(!finalize_root(&connection, "r", 0, "message-a", 5).expect("repeat"));
    }
}
