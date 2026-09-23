use std::sync::Arc;

use super::conversation_inputs::conversation_inputs_roles::CodexStepBinding;
use super::prepare::{compose_after_connect, FreshProviderContext};
use super::recovery::context_recovery_message;
use super::role_codex::{execute_role_codex_step, CodexStepRequest};
use super::role_steps::{
    await_premium_step, execute_specialist_request, parse_review_response, role_step_request,
};
use super::RoleCandidate;
use crate::ipc_contract::ConversationMessage;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    persist_conversation_success_with_state, AppState, RunCancellation, StartTurnInput,
    TurnExecutionFailure,
};

pub(super) async fn execute(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    mut role_candidates: Vec<RoleCandidate>,
    identity: &crate::CodexAgentRuntimeSettings,
    regional: &crate::persistence::settings::regional_preferences::RegionalPreferences,
    timeout_ms: u64,
    model: String,
    max_input_bytes: u32,
    binding: Option<CodexStepBinding>,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let binding = binding.ok_or_else(|| {
        TurnExecutionFailure::configuration(
            "Role-routing Codex dispatch has no persisted step binding",
        )
    })?;
    let FreshProviderContext { history, world, .. } = compose_after_connect(
        state,
        input,
        &identity,
        &regional,
        crate::runtime::context::broker::ProviderInputBudget::openai_compatible(),
    )
    .map_err(|error| TurnExecutionFailure::configuration(context_recovery_message(&error)))?;
    let result = execute_role_codex_step(
        state,
        input,
        on_event,
        cancellation.clone(),
        CodexStepRequest {
            step_id: binding.step_id.clone(),
            revision: binding.revision,
            config_fingerprint: binding.config_fingerprint.clone(),
            purpose: binding.purpose.clone(),
            model,
            max_input_bytes: max_input_bytes as usize,
            current_request: role_step_request(&binding.purpose, &input.content, &role_candidates)?,
        },
        &history,
        timeout_ms,
        world,
    )
    .await?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let usage_json = result.usage.as_ref().map(|usage| usage.as_json());
    let result_content = if binding.purpose == "tool_specialist" {
        execute_specialist_request(
            state,
            input,
            cancellation.as_ref(),
            &binding.step_id,
            i64::from(binding.revision),
            &binding.config_fingerprint,
            &result.content,
        )
        .await?
    } else {
        result.content
    };
    if binding.purpose == "review" {
        let response = parse_review_response(&result_content)?;
        let review_outcome = state.sqlite_writer.write(|connection| {
            crate::role_routing::repository::advance_review_step(
                connection,
                &input.run_id,
                &response,
                usage_json.as_deref(),
                now_ms,
            )
        })?;
        let draft = role_candidates
            .iter()
            .find(|candidate| matches!(candidate.purpose.as_str(), "respond" | "reconsider"))
            .cloned()
            .ok_or_else(|| {
                TurnExecutionFailure::configuration(
                    "Role-routing reviewed response lost its author draft",
                )
            })?;
        match review_outcome {
            crate::role_routing::repository::ReviewStepOutcome::Revise(decision) => {
                role_candidates.push(RoleCandidate {
                    step_id: binding.step_id,
                    purpose: "review".into(),
                    content: serde_json::to_string(&decision)
                        .map_err(|error| TurnExecutionFailure::configuration(error.to_string()))?,
                });
                return Box::pin(super::execute_conversation_turn_with_candidates(
                    state,
                    input,
                    on_event,
                    cancellation,
                    role_candidates,
                ))
                .await;
            }
            crate::role_routing::repository::ReviewStepOutcome::AwaitPremium(proposal) => {
                if await_premium_step(state, input, cancellation.clone(), &proposal).await? {
                    return Box::pin(super::execute_conversation_turn_with_candidates(
                        state,
                        input,
                        on_event,
                        cancellation,
                        role_candidates,
                    ))
                    .await;
                }
                return persist_conversation_success_with_state(
                    state,
                    input,
                    &draft.content,
                    |connection, message| {
                        crate::role_routing::repository::accept_reviewed_draft(
                            connection,
                            &input.run_id,
                            &draft.step_id,
                            &message.id,
                            now_ms,
                        )
                    },
                )
                .map_err(Into::into);
            }
            crate::role_routing::repository::ReviewStepOutcome::KeepDraft => {
                return persist_conversation_success_with_state(
                    state,
                    input,
                    &draft.content,
                    |connection, message| {
                        crate::role_routing::repository::accept_reviewed_draft(
                            connection,
                            &input.run_id,
                            &draft.step_id,
                            &message.id,
                            now_ms,
                        )
                    },
                )
                .map_err(Into::into);
            }
        }
    }
    if state.sqlite_writer.write(|connection| {
        crate::role_routing::repository::advance_provider_step_with_usage(
            connection,
            &input.run_id,
            &result_content,
            usage_json.as_deref(),
            now_ms,
        )
    })? {
        role_candidates.push(RoleCandidate {
            step_id: binding.step_id,
            purpose: binding.purpose,
            content: result_content,
        });
        return Box::pin(super::execute_conversation_turn_with_candidates(
            state,
            input,
            on_event,
            cancellation,
            role_candidates,
        ))
        .await;
    }
    persist_conversation_success_with_state(
        state,
        input,
        &result_content,
        |connection, message| {
            if let Some(usage_json) = usage_json.as_deref() {
                crate::role_routing::repository::record_step_usage(
                    connection,
                    &input.run_id,
                    usage_json,
                )?;
            }
            crate::role_routing::repository::accept_provider_turn(
                connection,
                &input.run_id,
                &message.id,
                now_ms,
            )
        },
    )
    .map_err(Into::into)
}
