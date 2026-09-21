//! Source-bound request classification. Model tool arguments never grant authority.
use super::{
    authority,
    contracts::GoalProposal,
    execution_contracts::{ProposeDecision, WorkProposeResult},
};
use crate::database_error;
use rusqlite::{params, Connection};
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
    if quoted_negative(&excerpt) || hypothetical(&excerpt) {
        return Ok(WorkProposeResult {
            decision: ProposeDecision::Rejected,
            proposal_id: None,
            goal_id: None,
            task_id: None,
            reason: "quoted_or_negative_source".into(),
            duplicate: false,
        });
    }
    if ambiguous(&excerpt) {
        return Ok(WorkProposeResult {
            decision: ProposeDecision::Clarify,
            proposal_id: None,
            goal_id: None,
            task_id: None,
            reason: "ambiguous_request".into(),
            duplicate: false,
        });
    }
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

fn quoted_negative(text: &str) -> bool {
    let lower = text.to_lowercase();
    let negative = [
        "しない",
        "しないで",
        "やめて",
        "撤回",
        "don't",
        "do not",
        "never",
    ];
    let quoted = text.contains('「') || text.contains('"') || text.contains('“');
    negative.iter().any(|marker| lower.contains(marker)) && (quoted || text.contains("と言"))
}

fn hypothetical(text: &str) -> bool {
    ["もし", "仮に", "suppose", "if we"]
        .iter()
        .any(|marker| text.to_lowercase().contains(marker))
        && !text.contains("実行して")
}

fn ambiguous(text: &str) -> bool {
    let trimmed = text.trim();
    matches!(trimmed, "それをお願い" | "お願い" | "do it" | "それ")
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
