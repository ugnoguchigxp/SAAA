//! Admission (docs/plans/worker-agents.md §5.2): one transaction binds a delegation to a persisted
//! user input and an offered, still-current profile revision, then records the task and its job.
use super::store::{self, db};
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::{load_revision, read_meta};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// `tx` must be a transaction (or a deref of one): the checks and the inserts commit together.
///
/// Every refusal the model can cause is a typed `Failed` outcome. `Err` means the request itself
/// is not admissible (the input message is not a persisted user message) or the database failed.
pub(crate) fn admit(
    tx: &Connection,
    req: &AdmitRequest<'_>,
    now_ms: i64,
) -> Result<WorkerOutcome, String> {
    // (a) The delegation is bound to a persisted user utterance of this conversation.
    let bound: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_messages
              WHERE id = ?1 AND conversation_id = ?2 AND role = 'user')",
            params![req.input_message_id, req.conversation_id],
            |row| row.get(0),
        )
        .map_err(db)?;
    if !bound {
        return Err("worker delegation requires a persisted user input message".into());
    }

    // (b) The agent must be an offered candidate of this decision (ranking is not authority).
    let candidate: Option<(String, i64, i64, String, Option<String>)> = tx
        .query_row(
            "SELECT c.profile_revision_id, d.registry_epoch, d.acl_epoch, d.conversation_id,
                    d.input_message_id
             FROM worker_discovery_candidates c
             JOIN worker_discovery_decisions d ON d.id = c.decision_id
             JOIN worker_profile_revisions r ON r.id = c.profile_revision_id
             WHERE c.decision_id = ?1 AND c.offered = 1 AND r.profile_id = ?2",
            params![req.decision_id, req.delegate.agent],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(db)?;
    let Some((revision_id, registry_epoch, acl_epoch, decision_conversation, decision_input)) =
        candidate
    else {
        return Ok(store::failed(None, FailureCode::NoMatchingAgent));
    };
    if decision_conversation != req.conversation_id
        || decision_input
            .as_deref()
            .is_some_and(|input| input != req.input_message_id)
    {
        return Ok(store::failed(None, FailureCode::NoMatchingAgent));
    }

    // (e) Identical delegation merges into the existing task, even if the registry moved on since.
    let canonical = store::canonical_json(&req.delegate.input);
    let idempotency_key = format!(
        "{:x}",
        Sha256::digest(
            format!("{}|{}|{}", req.input_message_id, revision_id, canonical).as_bytes()
        )
    );
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM worker_tasks WHERE idempotency_key = ?1",
            params![idempotency_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)?;
    if let Some(task_id) = existing {
        return Ok(store::deliver_sync(tx, &task_id)?.unwrap_or(WorkerOutcome::Pending { task_id }));
    }

    // (c) The offer must still describe the registry: epochs, enabled profile, current revision.
    let meta = read_meta(tx)?;
    if meta.registry_epoch != registry_epoch || meta.acl_epoch != acl_epoch {
        return Ok(store::failed(None, FailureCode::StaleOffer));
    }
    let Ok(revision) = load_revision(tx, &revision_id) else {
        return Ok(store::failed(None, FailureCode::StaleOffer));
    };
    let current: Option<(bool, Option<String>)> = tx
        .query_row(
            "SELECT enabled, current_revision_id FROM worker_profiles WHERE id = ?1",
            params![revision.profile_id],
            |row| Ok((row.get::<_, i64>(0)? == 1, row.get(1)?)),
        )
        .optional()
        .map_err(db)?;
    let still_current = matches!(
        &current,
        Some((true, Some(current_id))) if *current_id == revision_id
    );
    if !still_current || revision.review_state != ReviewState::Approved {
        return Ok(store::failed(None, FailureCode::StaleOffer));
    }
    // v1 executes read-only workers only; a write/unknown effect is never runnable.
    if !revision.all_tools_read_only() {
        return Ok(store::failed(None, FailureCode::ToolUnavailable));
    }

    // (d) The delegated input must satisfy the revision's input schema.
    let input_valid = jsonschema::validator_for(&revision.input_schema)
        .map(|validator| validator.is_valid(&req.delegate.input))
        .unwrap_or(false);
    if !input_valid {
        return Ok(store::failed(None, FailureCode::InputInvalid));
    }

    // (f) Record the task (pinning the revision) and its queue job in this transaction.
    let task_id = format!("wtask_{}", uuid::Uuid::new_v4().simple());
    let limits = &revision.limits;
    let deadline_at_ms =
        now_ms.saturating_add(i64::try_from(limits.deadline_ms).unwrap_or(i64::MAX));
    let conversation_room = (req.conversation_deadline_ms - now_ms - 5_000).max(0);
    let sync_wait = i64::try_from(limits.sync_wait_ms)
        .unwrap_or(i64::MAX)
        .min(conversation_room);
    tx.execute(
        "INSERT INTO worker_tasks(id, conversation_id, input_message_id, origin_job_key, decision_id,
            profile_id, profile_revision_id, idempotency_key, input_json, state, delivery,
            tier_index, attempts, restarts, deadline_at_ms, sync_wait_until_ms,
            created_at_ms, updated_at_ms)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'accepted', 'sync_waiting',
            0, 0, 0, ?10, ?11, ?12, ?12)",
        params![
            task_id,
            req.conversation_id,
            req.input_message_id,
            req.origin_job_key,
            req.decision_id,
            revision.profile_id,
            revision_id,
            idempotency_key,
            req.delegate.input.to_string(),
            deadline_at_ms,
            now_ms.saturating_add(sync_wait),
            now_ms,
        ],
    )
    .map_err(db)?;
    crate::task_queue::enqueue(
        tx,
        req.conversation_id,
        WORKER_LANE,
        WORKER_JOB_KIND,
        &task_id,
        0,
        &serde_json::json!({ "taskId": task_id }).to_string(),
        None,
    )?;
    Ok(WorkerOutcome::Pending { task_id })
}
