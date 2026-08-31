mod completion;
mod contracts;
mod persistence;
mod run_state;
#[cfg(test)]
mod tests;

use std::{collections::HashMap, sync::Mutex};

use rusqlite::{params, OptionalExtension};

use crate::{database_error, now_iso, validate_identifier, AppState, StartTurnInput};
pub(crate) use completion::{
    completion_state, effective_presentation, presentation_and_snapshot,
    upper_policies_allow_speech,
};
use contracts::{
    encode_tool_result, parse_tool_arguments, VoiceToolArguments, VoiceToolEffective,
    VoiceToolError, VoiceToolOutcomes, VoiceToolResult, VoiceToolTakesEffect,
};
pub(crate) use contracts::{
    tool_definition, ConversationVoicePolicySnapshot, ResetConversationVoicePolicyInput,
    UpdateConversationVoicePolicyInput, VoicePresentationDecision,
};
use persistence::{ensure_policy, load_policy, record_event, source_message_id};
pub(crate) use persistence::{
    migrate, policy_snapshot, reset_policy_from_ui, update_policy_from_ui,
};
use run_state::apply_ui_speech_runtime;

pub(crate) const UPDATE_VOICE_BEHAVIOR_TOOL_NAME: &str = "update_conversation_voice_behavior";
const QUICK_SILENCE_TIMEOUT_MS: u32 = 900;
const BALANCED_SILENCE_TIMEOUT_MS: u32 = 1_500;
const PATIENT_SILENCE_TIMEOUT_MS: u32 = 2_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunSpeechOverride {
    Silent,
    Speak,
}

#[derive(Debug, Clone)]
struct RunVoiceState {
    conversation_id: String,
    starting_policy_revision: i64,
    speech_override: Option<RunSpeechOverride>,
    mutation_count: usize,
    results: HashMap<String, String>,
}

#[derive(Default)]
pub(crate) struct VoiceBehaviorRuntime {
    runs: Mutex<HashMap<String, RunVoiceState>>,
}

#[tauri::command]
pub(crate) fn get_conversation_voice_policy(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<ConversationVoicePolicySnapshot, String> {
    policy_snapshot(&state, &conversation_id)
}

#[tauri::command]
pub(crate) fn update_conversation_voice_policy(
    state: tauri::State<'_, AppState>,
    input: UpdateConversationVoicePolicyInput,
) -> Result<ConversationVoicePolicySnapshot, String> {
    update_policy_from_ui(&state, input)
}

#[tauri::command]
pub(crate) fn reset_conversation_voice_policy(
    state: tauri::State<'_, AppState>,
    input: ResetConversationVoicePolicyInput,
) -> Result<ConversationVoicePolicySnapshot, String> {
    reset_policy_from_ui(&state, input)
}

pub(crate) fn begin_run(
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
) -> Result<bool, String> {
    validate_identifier(run_id, "run id")?;
    validate_identifier(conversation_id, "conversation id")?;
    let policy = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        let task_mode = connection
            .query_row(
                "SELECT task_mode FROM conversations WHERE id=?1",
                params![conversation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| "Conversation does not exist".to_string())?;
        if task_mode != "conversation" {
            return Ok(false);
        }
        ensure_policy(&connection, conversation_id)?
    };
    let mut runs = state
        .voice_behavior
        .runs
        .lock()
        .map_err(|_| "Voice behavior runtime lock unavailable".to_string())?;
    if runs.contains_key(run_id) {
        return Err("Voice behavior state already exists for this run".to_string());
    }
    runs.insert(
        run_id.to_string(),
        RunVoiceState {
            conversation_id: conversation_id.to_string(),
            starting_policy_revision: policy.policy_revision,
            speech_override: None,
            mutation_count: 0,
            results: HashMap::new(),
        },
    );
    Ok(true)
}

pub(crate) fn begin_turn_speech_policy(
    state: &AppState,
    input: &StartTurnInput,
) -> Result<(bool, bool), String> {
    let voice_behavior_run = begin_run(state, &input.run_id, &input.conversation_id)?;
    let policy = (|| {
        let streaming = if voice_behavior_run {
            upper_policies_allow_speech(state)?
        } else {
            input.presentation_mode == "visual-and-spoken"
        };
        let enabled = if voice_behavior_run {
            effective_presentation(state, Some(&input.run_id), &input.conversation_id)?.decision
                == "speak"
        } else {
            streaming
        };
        Ok((streaming, enabled))
    })();
    if policy.is_err() {
        end_run(state, &input.run_id);
    }
    policy
}

pub(crate) fn end_run(state: &AppState, run_id: &str) {
    if let Ok(mut runs) = state.voice_behavior.runs.lock() {
        runs.remove(run_id);
    }
}

pub(crate) fn execute_tool(
    state: &AppState,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
) -> String {
    if let Some(result) = cached_result(state, &input.run_id, &call.id) {
        return result;
    }
    let arguments = match parse_tool_arguments(&call.arguments) {
        Ok(arguments) => arguments,
        Err(()) => {
            let result = crate::runtime::agent_tools::tool_error_content(
                "invalid-input",
                "Tool arguments do not match the update_conversation_voice_behavior schema.",
            );
            cache_result(state, &input.run_id, &call.id, result.clone());
            return result;
        }
    };
    let fail_closed_silent = arguments
        .speech_output
        .as_ref()
        .is_some_and(|change| change.mode == "silent");
    if fail_closed_silent {
        state.streaming_tts.set_enabled(&input.run_id, false);
        let _ = set_run_override(state, &input.run_id, Some(RunSpeechOverride::Silent));
    }
    let (starting_revision, conversation_id) = match reserve_mutation(state, input, &call.id) {
        Ok(value) => value,
        Err(result) => return result,
    };

    let result = apply_tool_mutation(
        state,
        input,
        call,
        &conversation_id,
        starting_revision,
        &arguments,
    )
    .unwrap_or_else(|_| {
        crate::runtime::agent_tools::tool_error_content(
            "voice-policy-unavailable",
            "The conversation voice policy could not be updated.",
        )
    });
    cache_result(state, &input.run_id, &call.id, result.clone());
    result
}

pub(crate) fn execute_tool_for_state(
    state: Option<&AppState>,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
) -> String {
    let Some(state) = state else {
        return crate::runtime::agent_tools::tool_error_content(
            "voice-policy-unavailable",
            "The conversation voice policy is unavailable for this request.",
        );
    };
    execute_tool(state, input, call)
}

fn reserve_mutation(
    state: &AppState,
    input: &StartTurnInput,
    tool_call_id: &str,
) -> Result<(i64, String), String> {
    let mut runs = state.voice_behavior.runs.lock().map_err(|_| {
        crate::runtime::agent_tools::tool_error_content(
            "voice-policy-unavailable",
            "The conversation voice policy is temporarily unavailable.",
        )
    })?;
    let run = runs.get_mut(&input.run_id).ok_or_else(|| {
        crate::runtime::agent_tools::tool_error_content(
            "voice-policy-unavailable",
            "The conversation voice policy is not available for this run.",
        )
    })?;
    if run.conversation_id != input.conversation_id {
        return Err(crate::runtime::agent_tools::tool_error_content(
            "voice-policy-unavailable",
            "The conversation voice policy run context is invalid.",
        ));
    }
    if let Some(result) = run.results.get(tool_call_id) {
        return Err(result.clone());
    }
    if run.mutation_count >= 1 {
        return Err(crate::runtime::agent_tools::tool_error_content(
            "voice-policy-quota-exceeded",
            "Only one voice behavior change is allowed per turn.",
        ));
    }
    run.mutation_count += 1;
    Ok((run.starting_policy_revision, run.conversation_id.clone()))
}

fn apply_tool_mutation(
    state: &AppState,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
    conversation_id: &str,
    starting_revision: i64,
    arguments: &VoiceToolArguments,
) -> Result<String, String> {
    let policy_guard = state
        .interaction_policy
        .lock()
        .map_err(|_| "Interaction policy lock unavailable".to_string())?;
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    let current = load_policy(&transaction, conversation_id)?;
    let persistent_change = arguments
        .speech_output
        .as_ref()
        .is_some_and(|change| change.scope == "conversation")
        || arguments.listening_pace.is_some();
    if persistent_change && current.policy_revision != starting_revision {
        record_event(
            &transaction,
            conversation_id,
            Some(&input.run_id),
            Some(&call.id),
            source_message_id(&transaction, &input.run_id)?.as_deref(),
            "tool",
            &current,
            &current,
            "policy-conflict",
        )?;
        transaction.commit().map_err(database_error)?;
        drop(connection);
        let (presentation, snapshot) =
            presentation_and_snapshot(state, Some(&input.run_id), conversation_id)?;
        drop(policy_guard);
        return encode_tool_result(VoiceToolResult {
            applied: false,
            policy_revision: current.policy_revision,
            outcomes: VoiceToolOutcomes {
                speech_output: if arguments
                    .speech_output
                    .as_ref()
                    .is_some_and(|change| change.mode == "silent")
                {
                    "applied-current-response-only"
                } else if arguments.speech_output.is_some() {
                    "conflict"
                } else {
                    "unchanged"
                },
                listening_pace: if arguments.listening_pace.is_some() {
                    "conflict"
                } else {
                    "unchanged"
                },
            },
            effective: VoiceToolEffective {
                speech_output: presentation.decision,
                speech_reason_code: presentation.reason_code,
                listening_pace: snapshot.effective_listening_pace,
            },
            takes_effect: VoiceToolTakesEffect {
                speech_output: if arguments
                    .speech_output
                    .as_ref()
                    .is_some_and(|change| change.mode == "silent")
                {
                    "current_response"
                } else {
                    "unchanged"
                },
                listening_pace: "unchanged",
            },
            error: Some(VoiceToolError {
                code: "policy-conflict",
                message:
                    "The voice policy changed after this turn started. The newer setting was kept.",
            }),
        });
    }

    let mut next = current.clone();
    let mut run_override = None;
    if let Some(change) = &arguments.speech_output {
        match (change.mode.as_str(), change.scope.as_str()) {
            ("silent", "current_response") => run_override = Some(RunSpeechOverride::Silent),
            ("speak", "current_response") => run_override = Some(RunSpeechOverride::Speak),
            ("silent", "conversation") => {
                next.speech_output_override = "muted".to_string();
                run_override = Some(RunSpeechOverride::Silent);
            }
            ("inherit", "conversation") => {
                next.speech_output_override = "inherit".to_string();
            }
            _ => return Err("Invalid voice policy mutation".to_string()),
        }
    }
    if let Some(change) = &arguments.listening_pace {
        next.listening_pace_override = match change.mode.as_str() {
            "default" => "inherit".to_string(),
            value => value.to_string(),
        };
    }
    let speech_policy_changed = next.speech_output_override != current.speech_output_override;
    let listening_policy_changed = next.listening_pace_override != current.listening_pace_override;
    let changed = speech_policy_changed || listening_policy_changed;
    if changed {
        next.policy_revision += 1;
        next.updated_at = now_iso();
        let updated = transaction
            .execute(
                "UPDATE conversation_voice_policies
                 SET speech_output_override=?1, listening_pace_override=?2,
                     policy_revision=?3, updated_at=?4
                 WHERE conversation_id=?5 AND policy_revision=?6",
                params![
                    next.speech_output_override,
                    next.listening_pace_override,
                    next.policy_revision,
                    next.updated_at,
                    conversation_id,
                    current.policy_revision
                ],
            )
            .map_err(database_error)?;
        if updated != 1 {
            return Err("Voice policy changed while the tool update was being applied".to_string());
        }
    }
    record_event(
        &transaction,
        conversation_id,
        Some(&input.run_id),
        Some(&call.id),
        source_message_id(&transaction, &input.run_id)?.as_deref(),
        "tool",
        &current,
        &next,
        if changed || run_override.is_some() {
            "applied"
        } else {
            "unchanged"
        },
    )?;
    transaction.commit().map_err(database_error)?;
    drop(connection);
    if arguments.speech_output.is_some() {
        set_run_override(state, &input.run_id, run_override)?;
    }
    let (presentation, snapshot) =
        presentation_and_snapshot(state, Some(&input.run_id), conversation_id)?;
    state
        .streaming_tts
        .set_enabled(&input.run_id, presentation.decision == "speak");
    drop(policy_guard);
    encode_tool_result(VoiceToolResult {
        applied: true,
        policy_revision: next.policy_revision,
        outcomes: VoiceToolOutcomes {
            speech_output: outcome(
                arguments.speech_output.is_some(),
                speech_policy_changed || run_override.is_some(),
            ),
            listening_pace: outcome(arguments.listening_pace.is_some(), listening_policy_changed),
        },
        effective: VoiceToolEffective {
            speech_output: presentation.decision,
            speech_reason_code: presentation.reason_code,
            listening_pace: snapshot.effective_listening_pace,
        },
        takes_effect: VoiceToolTakesEffect {
            speech_output: if arguments.speech_output.is_some() {
                "current_response"
            } else {
                "unchanged"
            },
            listening_pace: if arguments.listening_pace.is_some() {
                "next_user_turn"
            } else {
                "unchanged"
            },
        },
        error: None,
    })
}

fn outcome(requested: bool, changed: bool) -> &'static str {
    if !requested {
        "unchanged"
    } else if changed {
        "applied"
    } else {
        "unchanged"
    }
}

fn set_run_override(
    state: &AppState,
    run_id: &str,
    value: Option<RunSpeechOverride>,
) -> Result<(), String> {
    let mut runs = state
        .voice_behavior
        .runs
        .lock()
        .map_err(|_| "Voice behavior runtime lock unavailable".to_string())?;
    let run = runs
        .get_mut(run_id)
        .ok_or_else(|| "Voice behavior state is not available for this run".to_string())?;
    run.speech_override = value;
    Ok(())
}

fn cached_result(state: &AppState, run_id: &str, tool_call_id: &str) -> Option<String> {
    state
        .voice_behavior
        .runs
        .lock()
        .ok()
        .and_then(|runs| runs.get(run_id)?.results.get(tool_call_id).cloned())
}

fn cache_result(state: &AppState, run_id: &str, tool_call_id: &str, result: String) {
    if let Ok(mut runs) = state.voice_behavior.runs.lock() {
        if let Some(run) = runs.get_mut(run_id) {
            run.results.insert(tool_call_id.to_string(), result);
        }
    }
}
