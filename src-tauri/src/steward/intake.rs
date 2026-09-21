//! Source-bound request classification. Model tool arguments never grant authority.
use super::{
    authority,
    contracts::GoalProposal,
    execution_contracts::{ProposeDecision, WorkProposeResult},
};
use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RawProposalArgs {
    pub workspace_id: String,
    pub summary: String,
    pub success_condition: super::contracts::Verifier,
    pub operations: Vec<super::contracts::Operation>,
    pub budget_runs: u8,
    pub budget_ms: u64,
    #[serde(default)]
    pub notify: Option<super::contracts::Notify>,
    #[serde(default)]
    pub quote_start: Option<u32>,
    #[serde(default)]
    pub quote_end: Option<u32>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub recipe_id: Option<String>,
    #[serde(default)]
    pub source_message_id: Option<String>,
}

pub(crate) fn classify(
    connection: &Connection,
    conversation_id: &str,
    bound_source_id: &str,
    proposal: &GoalProposal,
) -> Result<WorkProposeResult, String> {
    proposal.validate()?;
    if proposal.source_message_id != bound_source_id {
        return Ok(WorkProposeResult {
            decision: ProposeDecision::Rejected,
            proposal_id: None,
            goal_id: None,
            task_id: None,
            reason: "source_mismatch".into(),
            duplicate: false,
        });
    }
    let (binding, excerpt) = authority::binding_from_message(
        connection,
        conversation_id,
        bound_source_id,
        proposal.quote_start,
        proposal.quote_end,
    )?;
    let full_text = connection
        .query_row(
            "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2",
            rusqlite::params![bound_source_id, conversation_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| "source_unavailable".to_string())?;
    let intents = super::request_intent::classify_source(&full_text, &proposal.operations);
    let decision = super::request_intent::overall(&intents);
    if decision != super::request_intent::IntentDecision::Allowed {
        return Ok(WorkProposeResult {
            decision: match decision {
                super::request_intent::IntentDecision::Ambiguous => ProposeDecision::Clarify,
                _ => ProposeDecision::Rejected,
            },
            proposal_id: None,
            goal_id: None,
            task_id: None,
            reason: super::request_intent::reason_for(decision).into(),
            duplicate: false,
        });
    }
    let _ = excerpt;
    if let Some(grant) = authority::covering_grant(connection, conversation_id, proposal)? {
        return super::admission::admit_existing(
            connection,
            conversation_id,
            proposal,
            &binding,
            &grant,
        );
    }
    super::admission::stage_confirmation(connection, conversation_id, proposal, &binding)
}

pub(crate) fn proposal_digest(proposal: &GoalProposal) -> String {
    let payload = format!(
        "{}|{}|{}|{}|{:?}|{:?}",
        proposal.workspace_id,
        proposal.verifier_key(),
        proposal.operations_key(),
        proposal.budget_runs,
        proposal.target,
        proposal.recipe_id
    );
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}

pub(crate) fn load_proposal(
    connection: &Connection,
    proposal_id: &str,
) -> Result<(i64, String, String, String), String> {
    connection
        .query_row(
            "SELECT revision,digest,payload_json,status FROM steward_proposals WHERE id=?1",
            [proposal_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| "proposal_unavailable".to_string())
}

pub(crate) fn confirm(
    connection: &Connection,
    conversation_id: &str,
    proposal_id: &str,
    expected_revision: i64,
    display_digest: &str,
    start: bool,
) -> Result<WorkProposeResult, String> {
    let (revision, digest, payload, status): (i64, String, String, String) =
        load_proposal(connection, proposal_id)?;
    if status != "pending" || revision != expected_revision || digest != display_digest {
        return Err("stale_confirmation".into());
    }
    let proposal: GoalProposal =
        serde_json::from_str(&payload).map_err(|_| "proposal_unavailable")?;
    if proposal.workspace_id.is_empty() {
        return Err("stale_confirmation".into());
    }
    let bound_version: Option<String> = connection
        .query_row(
            "SELECT source_version FROM steward_source_bindings
             WHERE subject_kind='proposal' AND subject_id=?1
             ORDER BY rowid DESC LIMIT 1",
            [proposal_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    let current =
        authority::current_source_digest(connection, conversation_id, &proposal.source_message_id)?;
    if bound_version.as_deref() != Some(current.as_str()) {
        return Err("stale_confirmation".into());
    }
    let full_text: String = connection
        .query_row(
            "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2",
            rusqlite::params![proposal.source_message_id, conversation_id],
            |row| row.get(0),
        )
        .map_err(|_| "source_unavailable".to_string())?;
    let intents = super::request_intent::classify_source(&full_text, &proposal.operations);
    if super::request_intent::overall(&intents) != super::request_intent::IntentDecision::Allowed {
        return Err("stale_confirmation".into());
    }
    let receipt = crate::new_id("ui_receipt");
    connection
        .execute(
            "UPDATE steward_proposals SET status='confirmed',confirmation_receipt=?2 WHERE id=?1",
            params![proposal_id, receipt],
        )
        .map_err(database_error)?;
    let binding = authority::SourceBinding {
        kind: authority::SourceKind::UiReceipt,
        id: receipt,
        version: digest.clone(),
        quote_start: None,
        quote_end: None,
    };
    super::admission::admit_confirmed(connection, conversation_id, &proposal, &binding, start)
}
