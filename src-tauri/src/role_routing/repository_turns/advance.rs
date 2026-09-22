use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::ledger::{append_event, record_provider_outcome};
use super::lifecycle::record_step_usage;

/// Commits a non-final provider candidate and claims the next planned step atomically. The draft
/// body remains process-local; the routing ledger stores only a digest and byte count. Returns
/// `false` without mutation when the active step is the final step, leaving final adoption to the
/// assistant-message transaction.
pub(crate) fn advance_provider_step(
    connection: &Connection,
    run_id: &str,
    content: &str,
    now_ms: i64,
) -> Result<bool, String> {
    advance_provider_step_with_usage(connection, run_id, content, None, now_ms)
}

pub(crate) fn advance_provider_step_with_usage(
    connection: &Connection,
    run_id: &str,
    content: &str,
    usage_json: Option<&str>,
    now_ms: i64,
) -> Result<bool, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let root: Option<(i64, i64, String)> = transaction
        .query_row(
            "SELECT revision,cancel_requested,phase FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((revision, cancel_requested, phase)) = root else {
        return Ok(false);
    };
    if phase != "responding" || cancel_requested != 0 {
        return Err("Role-routing intermediate result is not adoptable".into());
    }
    let active_step: Option<(String, i64)> = transaction
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((step_id, step_revision)) = active_step else {
        return Err("Role-routing intermediate result has no active step".into());
    };
    if step_revision != revision {
        return Err("Role-routing intermediate result is stale".into());
    }
    let has_next: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_steps current JOIN rr_steps next ON next.root_id=current.root_id AND next.revision=current.revision AND next.ordinal>current.ordinal AND next.status='planned' WHERE current.id=?1 AND current.root_id=?2)",
            params![step_id, run_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_next {
        return Ok(false);
    }
    if let Some(usage_json) = usage_json {
        record_step_usage(&transaction, run_id, usage_json)?;
    }
    if !crate::role_routing::steps::complete_step(
        &transaction,
        run_id,
        &step_id,
        "succeeded",
        now_ms,
    )? {
        return Err("Role-routing intermediate step was already completed".into());
    }
    let digest = format!("{:x}", Sha256::digest(content.as_bytes()));
    crate::role_routing::steps::record_step_output(
        &transaction,
        run_id,
        &step_id,
        revision,
        "intermediate",
        &json!({"sha256": digest, "bytes": content.len()}).to_string(),
        false,
        now_ms,
    )?;
    crate::role_routing::steps::claim_next_planned_step(&transaction, run_id, revision, now_ms)?
        .ok_or_else(|| "Role-routing next step disappeared before claim".to_string())?;
    append_event(&transaction, run_id, "step_completed", now_ms)?;
    append_event(&transaction, run_id, "step_started", now_ms)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(true)
}

#[derive(Debug)]
pub(crate) enum ReviewStepOutcome {
    Revise(crate::role_routing::revision::ReviewRevisionDecision),
    AwaitPremium(crate::role_routing::proposals::ProposalReceipt),
    KeepDraft,
}

/// Persists a typed review and its host decision, then atomically claims or cancels revision.
pub(crate) fn advance_review_step(
    connection: &Connection,
    run_id: &str,
    response: &crate::role_routing::review::ReviewResponse,
    usage_json: Option<&str>,
    now_ms: i64,
) -> Result<ReviewStepOutcome, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let (revision, cancel_requested, phase, policy_id, policy_json, root_deadline): (
        i64,
        i64,
        String,
        String,
        String,
        Option<i64>,
    ) = transaction
        .query_row(
            "SELECT r.revision,r.cancel_requested,r.phase,r.policy_id,p.config_json,r.deadline_at_ms FROM rr_roots r JOIN rr_policy_versions p ON p.id=r.policy_id WHERE r.root_id=?1",
            [run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(|_| "Role-routing review root is unavailable".to_string())?;
    if phase != "responding" || cancel_requested != 0 {
        return Err("Role-routing review is not adoptable".into());
    }
    let (step_id, step_revision): (String, i64) = transaction
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' AND purpose='review'",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "Role-routing review has no active step".to_string())?;
    if step_revision != revision {
        return Err("Role-routing review is stale".into());
    }
    if let Some(usage_json) = usage_json {
        transaction
            .execute(
                "UPDATE rr_steps SET usage_json=?1 WHERE id=?2 AND root_id=?3 AND revision=?4 AND status='running'",
                params![usage_json, step_id, run_id, revision],
            )
            .map_err(|error| error.to_string())?;
    }
    crate::role_routing::repository::record_review_response(
        &transaction,
        &step_id,
        response,
        now_ms,
    )?;
    let review_output_id: String = transaction
        .query_row(
            "SELECT id FROM rr_outputs WHERE step_id=?1 AND revision=?2 AND kind='review' AND accepted=1",
            params![step_id, revision],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let policy: crate::role_routing::RoleRoutingSettings = serde_json::from_str(&policy_json)
        .map_err(|_| "Stored role-routing policy is invalid".to_string())?;
    let decision = crate::role_routing::repository::record_review_revision_decision(
        &transaction,
        &review_output_id,
        policy.limits.max_review_rounds,
        now_ms,
    )?;
    if !crate::role_routing::steps::complete_step(
        &transaction,
        run_id,
        &step_id,
        "succeeded",
        now_ms,
    )? {
        return Err("Role-routing review step was already completed".into());
    }
    let outcome = if decision.revision_allowed {
        crate::role_routing::steps::claim_next_planned_step(
            &transaction,
            run_id,
            revision,
            now_ms,
        )?
        .ok_or_else(|| "Role-routing revise step disappeared before claim".to_string())?;
        append_event(&transaction, run_id, "step_started", now_ms)?;
        ReviewStepOutcome::Revise(decision)
    } else {
        crate::role_routing::steps::settle_unfinished_steps(
            &transaction,
            run_id,
            revision,
            "cancelled",
            now_ms,
        )?;
        let premium = policy
            .roles
            .premium
            .as_deref()
            .filter(|_| policy.premium_approval == "per_request")
            .filter(|_| !decision.unresolved_issues.is_empty())
            // A metered cloud step with no trusted quote must not be proposed under a cost cap.
            .filter(|_| policy.limits.max_estimated_cost_micros.is_none());
        if let Some(candidate_id) = premium {
            let expires_at_ms = root_deadline
                .unwrap_or_else(|| now_ms.saturating_add(60_000))
                .min(now_ms.saturating_add(60_000));
            if expires_at_ms > now_ms {
                let receipt = crate::role_routing::proposals::record(
                    &transaction,
                    &crate::role_routing::proposals::Proposal {
                        id: format!("rr-proposal-{run_id}-{revision}"),
                        root_id: run_id.to_string(),
                        candidate_id: candidate_id.to_string(),
                        policy_id,
                        revision: u32::try_from(revision)
                            .map_err(|_| "Role-routing revision is invalid".to_string())?,
                        expires_at_ms,
                    },
                    None,
                    now_ms,
                )?;
                append_event(&transaction, run_id, "premium_proposed", now_ms)?;
                ReviewStepOutcome::AwaitPremium(receipt)
            } else {
                ReviewStepOutcome::KeepDraft
            }
        } else {
            ReviewStepOutcome::KeepDraft
        }
    };
    append_event(&transaction, run_id, "review_completed", now_ms)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(outcome)
}

/// Adopts the original process-local draft after review found no verified reason to revise.
pub(crate) fn accept_reviewed_draft(
    connection: &Connection,
    run_id: &str,
    draft_step_id: &str,
    message_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let (revision, phase, cancelled): (i64, String, i64) = connection
        .query_row(
            "SELECT revision,phase,cancel_requested FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "Role-routing reviewed root is unavailable".to_string())?;
    if phase != "responding" || cancelled != 0 {
        return Err("Role-routing reviewed draft is not adoptable".into());
    }
    let eligible: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_steps d WHERE d.id=?1 AND d.root_id=?2 AND d.revision=?3 AND d.status='succeeded' AND d.purpose IN ('respond','reconsider') AND EXISTS(SELECT 1 FROM rr_steps v WHERE v.root_id=d.root_id AND v.revision=d.revision AND v.purpose='review' AND v.status='succeeded') AND NOT EXISTS(SELECT 1 FROM rr_steps x WHERE x.root_id=d.root_id AND x.revision=d.revision AND x.status IN ('planned','running','draining')))",
            params![draft_step_id, run_id, revision],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !eligible {
        return Err("Role-routing reviewed draft receipt is incomplete".into());
    }
    if !crate::role_routing::steps::finalize_root(connection, run_id, revision, message_id, now_ms)?
    {
        return Err("Role-routing reviewed draft was already accepted".into());
    }
    connection
        .execute(
            "INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,?3,'answer',?4,1,?5)",
            params![format!("rr-final-{draft_step_id}"), draft_step_id, revision, json!({"messageId": message_id}).to_string(), now_ms],
        )
        .map_err(|error| error.to_string())?;
    append_event(connection, run_id, "answer_committed", now_ms)?;
    record_provider_outcome(connection, run_id, true, message_id, revision, now_ms)?;
    crate::role_routing::learning::repository::mark_root_dirty(connection, run_id)?;
    Ok(())
}
