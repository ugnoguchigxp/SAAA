//! Model-facing delegated-work tools.
//!
//! The model may describe a proposal, but it cannot nominate or fabricate the
//! source message that authorizes it.  The host binds every proposal to the
//! persisted user input for the current runtime turn.
use super::{
    contracts::{GoalProposal, Notify, Operation, Verifier},
    repository as repo,
};
use crate::{AppState, StartTurnInput};
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) const NAMES: [&str; 4] = ["work_propose", "work_status", "work_amend", "work_withdraw"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Propose {
    workspace_id: String,
    summary: String,
    success_condition: Verifier,
    operations: Vec<Operation>,
    budget_runs: u8,
    budget_ms: u64,
    #[serde(default)]
    notify: Option<Notify>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoalRef {
    goal_id: String,
}

pub(crate) fn definitions() -> Vec<Value> {
    let id = json!({"type":"string","minLength":1,"maxLength":160});
    vec![
        json!({"type":"function","function":{"name":"work_propose","description":"Propose a bounded read/test background task from the current explicit user request. This creates a proposal/authority record only; it does not claim that work started or completed. Never use it for write, shell, or network work.","parameters":{"type":"object","additionalProperties":false,"properties":{"workspaceId":id,"summary":{"type":"string","minLength":1,"maxLength":2000},"successCondition":{"type":"string","enum":["test_report_obtained","tests_pass","user_confirmation_required"]},"operations":{"type":"array","minItems":1,"maxItems":2,"items":{"type":"string","enum":["read","test_run"]}},"budgetRuns":{"type":"integer","minimum":1,"maximum":16},"budgetMs":{"type":"integer","minimum":1,"maximum":3600000},"notify":{"type":"string","enum":["both","silent","speak"]}},"required":["workspaceId","summary","successCondition","operations","budgetRuns","budgetMs"]}}}),
        json!({"type":"function","function":{"name":"work_status","description":"List the durable delegated-work state for this conversation.","parameters":{"type":"object","additionalProperties":false,"properties":{}}}}),
        json!({"type":"function","function":{"name":"work_withdraw","description":"Withdraw exactly one delegated goal when the user explicitly asks. This requests cancellation but cannot erase effects already performed.","parameters":{"type":"object","additionalProperties":false,"properties":{"goalId":id},"required":["goalId"]}}}),
        json!({"type":"function","function":{"name":"work_amend","description":"Change only notification delivery for an existing delegated goal. Do not use it to widen operations or budget.","parameters":{"type":"object","additionalProperties":false,"properties":{"goalId":id,"notify":{"type":"string","enum":["both","silent","speak"]}},"required":["goalId","notify"]}}}),
    ]
}

pub(crate) fn execute(
    state: Option<&AppState>,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
) -> String {
    let result = state.ok_or("work_unavailable".to_string()).and_then(|state| match call.name.as_str() {
        "work_propose" => {
            let proposal: Propose = serde_json::from_str(&call.arguments).map_err(|_| "work_proposal_invalid")?;
            let source_message_id = state.sqlite_readers.read(|c| repo::input_message_id(c, &input.run_id))?
                .ok_or("source_unavailable")?;
            let proposal = GoalProposal {
                source_message_id,
                workspace_id: proposal.workspace_id,
                summary: proposal.summary,
                success_condition: proposal.success_condition,
                operations: proposal.operations,
                budget_runs: proposal.budget_runs,
                budget_ms: proposal.budget_ms,
                notify: proposal.notify.unwrap_or(Notify::Both),
                quote_start: None,
                quote_end: None,
                target: None,
                recipe_id: None,
            };
            let value = state.sqlite_writer.transact(|c| {
                repo::propose(c, &input.conversation_id, &proposal)
            })?;
            if value["decision"] == "accepted" {
                let _ = super::reduce::start_queued(state, input);
            }
            Ok(value)
        }
        "work_status" => state.sqlite_readers.read(|c| repo::list(c, &input.conversation_id)),
        "work_withdraw" => {
            let goal: GoalRef = serde_json::from_str(&call.arguments).map_err(|_| "work_withdraw_invalid")?;
            state.sqlite_writer.write(|c| {
                super::commands::withdraw_specified(c, &input.conversation_id, &goal.goal_id)
            })
        }
        "work_amend" => {
            #[derive(Deserialize)] #[serde(rename_all = "camelCase", deny_unknown_fields)] struct Amend { goal_id: String, notify: String }
            let amend: Amend = serde_json::from_str(&call.arguments).map_err(|_| "work_amend_invalid")?;
            if !matches!(amend.notify.as_str(), "both" | "silent" | "speak") {
                return Err("work_amend_invalid".into());
            }
            state.sqlite_writer.write(|c| {
                let changed = c.execute("UPDATE steward_delegations SET notify=?1,revision=revision+1 WHERE goal_id=?2 AND conversation_id=?3 AND status='active' AND superseded_by IS NULL", rusqlite::params![amend.notify, amend.goal_id, input.conversation_id]).map_err(crate::database_error)?;
                if changed == 0 { return Err("goal_unavailable".into()); }
                let revision: i64 = c.query_row(
                    "SELECT revision FROM steward_delegations WHERE goal_id=?1 AND conversation_id=?2 AND status='active' AND superseded_by IS NULL",
                    rusqlite::params![amend.goal_id, input.conversation_id],
                    |row| row.get(0),
                ).map_err(crate::database_error)?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or(0);
                crate::adaptive_improvement::set_override(
                    c,
                    crate::adaptive_improvement::Domain::Notification,
                    &amend.goal_id,
                    &amend.notify,
                    &format!("steward-amend:{}", amend.goal_id),
                    revision,
                    None,
                    now,
                )?;
                Ok(json!({"goalId":amend.goal_id,"status":"active","revisioned":true}))
            })
        }
        _ => Err("invalid_tool".into()),
    });
    match result {
        Ok(value) => value.to_string(),
        Err(code) => crate::runtime::agent_tools::tool_error_content(&code, &code),
    }
}
