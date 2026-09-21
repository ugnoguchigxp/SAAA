//! Atomic Goal/plan/task/reservation/intent admission.
use super::{
    authority::{self, ExistingGrant, SourceBinding},
    contracts::GoalProposal,
    execution_contracts::{ProposeDecision, WorkProposeResult},
    repository as repo,
};
use crate::{database_error, new_id, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

pub(crate) fn stage_confirmation(
    connection: &Connection,
    conversation_id: &str,
    proposal: &GoalProposal,
    binding: &SourceBinding,
) -> Result<WorkProposeResult, String> {
    crate::steward::faults::maybe("admission_before_rows")?;
    let digest = super::intake::proposal_digest(proposal);
    if let Some((id, revision)) = connection
        .query_row(
            "SELECT id,revision FROM steward_proposals WHERE source_message_id=?1 AND digest=?2",
            params![proposal.source_message_id, digest],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(database_error)?
    {
        return Ok(WorkProposeResult {
            decision: ProposeDecision::RequiresConfirmation,
            proposal_id: Some(id),
            goal_id: None,
            task_id: None,
            reason: "pending_confirmation".into(),
            duplicate: true,
        });
    }
    if incomplete_goal_count(connection, conversation_id)?
        >= super::contracts::MAX_ACTIVE_GOALS as i64
    {
        return Err("active_goal_limit".into());
    }
    let id = new_id("proposal");
    connection
        .execute(
            "INSERT INTO steward_proposals(id,conversation_id,source_message_id,revision,digest,payload_json,status,created_at)
             VALUES(?1,?2,?3,1,?4,?5,'pending',?6)",
            params![
                id,
                conversation_id,
                proposal.source_message_id,
                digest,
                serde_json::to_string(proposal).map_err(|_| "work_proposal_invalid")?,
                now_iso()
            ],
        )
        .map_err(database_error)?;
    authority::persist_binding(connection, "proposal", &id, binding, None, None, None, None)?;
    crate::steward::faults::maybe("admission_after_proposal")?;
    Ok(WorkProposeResult {
        decision: ProposeDecision::RequiresConfirmation,
        proposal_id: Some(id),
        goal_id: None,
        task_id: None,
        reason: "new_grant_requires_confirmation".into(),
        duplicate: false,
    })
}

pub(crate) fn admit_existing(
    connection: &Connection,
    conversation_id: &str,
    proposal: &GoalProposal,
    binding: &SourceBinding,
    grant: &ExistingGrant,
) -> Result<WorkProposeResult, String> {
    crate::steward::faults::maybe("admission_before_rows")?;
    let key = dedupe_key(grant, proposal);
    if let Some((goal_id, task_id)) = existing_receipt(connection, &key)? {
        return Ok(WorkProposeResult {
            decision: ProposeDecision::Accepted,
            proposal_id: None,
            goal_id: Some(goal_id),
            task_id,
            reason: "idempotent_replay".into(),
            duplicate: true,
        });
    }
    let work = repo::active_goal_work(connection, conversation_id, &grant.goal_id)?
        .ok_or("goal_unavailable")?;
    let task_id = repo::queue_task(
        connection,
        &work,
        conversation_id,
        &proposal.source_message_id,
        "admission",
    )?;
    authority::persist_binding(
        connection,
        "goal",
        &grant.goal_id,
        binding,
        None,
        None,
        Some(1),
        Some(grant.revision),
    )?;
    crate::steward::faults::maybe("admission_after_rows")?;
    Ok(WorkProposeResult {
        decision: ProposeDecision::Accepted,
        proposal_id: None,
        goal_id: Some(grant.goal_id.clone()),
        task_id,
        reason: "existing_grant".into(),
        duplicate: false,
    })
}

pub(crate) fn admit_confirmed(
    connection: &Connection,
    conversation_id: &str,
    proposal: &GoalProposal,
    binding: &SourceBinding,
    start: bool,
) -> Result<WorkProposeResult, String> {
    if let Some(existing) = idempotent_origin(connection, proposal)? {
        return Ok(existing);
    }
    if incomplete_goal_count(connection, conversation_id)?
        >= super::contracts::MAX_ACTIVE_GOALS as i64
    {
        if let Some(existing) = idempotent_origin(connection, proposal)? {
            return Ok(existing);
        }
        return Err("active_goal_limit".into());
    }
    crate::steward::faults::maybe("admission_before_rows")?;
    let registered = repo::register_with_options(
        connection,
        conversation_id,
        &proposal.workspace_id,
        proposal.summary.trim(),
        proposal.summary.trim(),
        proposal.verifier_key(),
        proposal.operations_key(),
        proposal.budget_runs,
        proposal.budget_ms,
        proposal.notify_key(),
    )?;
    let goal_id = registered["goalId"]
        .as_str()
        .ok_or("work_proposal_invalid")?;
    let digest = format!(
        "{}:{}:{}:{}",
        proposal.workspace_id,
        proposal.operations_key(),
        proposal.verifier_key(),
        proposal.target.clone().unwrap_or_default()
    );
    connection
        .execute(
            "INSERT INTO steward_origin_bindings(id,goal_id,origin_kind,origin_id,operation_digest,created_at)
             VALUES(?1,?2,'user_turn',?3,?4,?5)",
            params![
                new_id("origin"),
                goal_id,
                proposal.source_message_id,
                digest,
                now_iso()
            ],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE steward_delegations SET target=?2,recipe_id=?3 WHERE goal_id=?1",
            params![goal_id, proposal.target, proposal.recipe_id],
        )
        .map_err(database_error)?;
    authority::persist_binding(
        connection,
        "goal",
        goal_id,
        binding,
        None,
        None,
        Some(1),
        Some(1),
    )?;
    let mut task_id = None;
    if start {
        let work = repo::active_goal_work(connection, conversation_id, goal_id)?
            .ok_or("goal_unavailable")?;
        task_id = repo::queue_task(
            connection,
            &work,
            conversation_id,
            &proposal.source_message_id,
            "admission",
        )?;
    }
    crate::steward::faults::maybe("admission_after_rows")?;
    Ok(WorkProposeResult {
        decision: ProposeDecision::Accepted,
        proposal_id: None,
        goal_id: Some(goal_id.into()),
        task_id,
        reason: if start {
            "confirmed_start"
        } else {
            "grant_only"
        }
        .into(),
        duplicate: false,
    })
}

pub(crate) fn incomplete_goal_count(
    connection: &Connection,
    conversation_id: &str,
) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM steward_goals g
             LEFT JOIN steward_goal_progress p ON p.goal_id=g.id
             WHERE g.conversation_id=?1 AND g.status='active' AND g.superseded_by IS NULL
               AND COALESCE(p.work_status,'queued') NOT IN ('done','failed','cancelled')",
            [conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)
}

fn dedupe_key(grant: &ExistingGrant, proposal: &GoalProposal) -> String {
    format!(
        "{}:{}:{}:{}",
        grant.delegation_id,
        proposal.verifier_key(),
        proposal.target.clone().unwrap_or_default(),
        proposal.recipe_id.clone().unwrap_or_default()
    )
}

fn existing_receipt(
    connection: &Connection,
    key: &str,
) -> Result<Option<(String, Option<String>)>, String> {
    connection
        .query_row(
            "SELECT d.goal_id,t.id FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id
             WHERE t.dedupe_key=?1 LIMIT 1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)
}

fn idempotent_origin(
    connection: &Connection,
    proposal: &GoalProposal,
) -> Result<Option<WorkProposeResult>, String> {
    let digest = format!(
        "{}:{}:{}:{}",
        proposal.workspace_id,
        proposal.operations_key(),
        proposal.verifier_key(),
        proposal.target.clone().unwrap_or_default()
    );
    let Some(goal_id) = connection
        .query_row(
            "SELECT goal_id FROM steward_origin_bindings WHERE origin_kind='user_turn' AND origin_id=?1 AND operation_digest=?2",
            params![proposal.source_message_id, digest],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
    else {
        return Ok(None);
    };
    Ok(Some(WorkProposeResult {
        decision: ProposeDecision::Accepted,
        proposal_id: None,
        goal_id: Some(goal_id),
        task_id: None,
        reason: "idempotent_replay".into(),
        duplicate: true,
    }))
}

pub(crate) fn json_result(result: WorkProposeResult) -> serde_json::Value {
    json!({
        "decision": result.decision,
        "proposalId": result.proposal_id,
        "goalId": result.goal_id,
        "taskId": result.task_id,
        "reason": result.reason,
        "duplicate": result.duplicate,
        "requiresConfirmation": matches!(result.decision, ProposeDecision::RequiresConfirmation),
        "status": match result.decision {
            ProposeDecision::Accepted => "active",
            ProposeDecision::RequiresConfirmation => "pending_confirmation",
            ProposeDecision::Clarify => "clarify",
            ProposeDecision::Rejected => "rejected",
        }
    })
}
