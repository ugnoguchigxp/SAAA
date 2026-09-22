//! Role-routed Codex actor and final-wire World receipt.
use super::*;
#[path = "conversation_role_codex_prompt.rs"]
mod prompt;
pub(super) use prompt::role_codex_prompt;
use prompt::{output_schema_for_purpose, validate_step_binding};

#[derive(Debug, Clone)]
pub(super) struct CodexStepRequest {
    pub(super) step_id: String,
    pub(super) revision: u32,
    pub(super) config_fingerprint: String,
    pub(super) purpose: String,
    pub(super) model: String,
    pub(super) max_input_bytes: usize,
    pub(super) current_request: String,
}

pub(super) struct CodexStepResult {
    pub(super) content: String,
    pub(super) usage: Option<crate::role_routing::adapters::codex_protocol::SidecarUsage>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_role_codex_step(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    step: CodexStepRequest,
    history: &[ConversationMessage],
    timeout_ms: u64,
    world: Option<crate::runtime::context::world::turn::WorldLive>,
) -> Result<CodexStepResult, TurnExecutionFailure> {
    validate_step_binding(state, input, &step)?;
    let CodexStepRequest {
        step_id,
        revision: _,
        config_fingerprint: _,
        purpose,
        model,
        max_input_bytes,
        current_request,
    } = step;
    let request = crate::role_routing::adapters::codex::SidecarRequest {
        id: input.run_id.clone(),
        step_id,
        model,
        prompt: role_codex_prompt(history, &current_request, max_input_bytes)?,
        output_schema: output_schema_for_purpose(&purpose),
        timeout_ms: timeout_ms.clamp(1_000, 300_000),
    };
    crate::update_runtime_provider(state, &input.run_id, "codex-sdk")?;
    let _ = on_event.send(RuntimeEvent::Started {
        run_id: input.run_id.clone(),
        route: "conversation.respond".into(),
        provider_id: "codex-sdk".into(),
    });
    let writer = state.sqlite_writer.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        use crate::runtime::context::generation::{begin_with_writer, BeginGeneration};
        let mut generation = None;
        let outcome = crate::role_routing::adapters::codex::run_observed(
            &request,
            &cancellation,
            &mut |wire| {
                if let Some(world) = &world {
                    if let Some((old, new)) = world.refresh_blocks()? {
                        let prompt = wire["prompt"].as_str().ok_or("Codex prompt missing")?;
                        if prompt.matches(&old.with_world).count() != 1 {
                            return Err("Codex World missing from final prompt".into());
                        }
                        wire["prompt"] = serde_json::Value::String(prompt.replacen(
                            &old.with_world,
                            &new.with_world,
                            1,
                        ));
                    }
                    if !world.revalidate_current() {
                        return Err("Codex World expired before dispatch".into());
                    }
                }
                let bytes = serde_json::to_vec(wire).map_err(|e| e.to_string())?;
                if bytes.len() > max_input_bytes.min(48 * 1024) {
                    return Err("Codex World exceeds input budget".into());
                }
                let receipt = begin_with_writer(
                    writer.clone(),
                    BeginGeneration {
                        run_id: &request.id,
                        provider_session_id: None,
                        provider_id: Some("codex-sdk"),
                        purpose: "reasoning",
                        request_payload: &bytes,
                        envelope_payload: &bytes,
                        current_instruction_count: 1,
                    },
                )?;
                receipt.set_health("green")?;
                if let Some(world) = &world {
                    world.bind(&receipt);
                    if let Some(candidate) = world.current_candidate() {
                        receipt.add_input(
                            "world-model",
                            &candidate.source_id,
                            candidate.source_version,
                            &candidate.source_digest,
                            "may",
                            "reference",
                            true,
                            None,
                        )?;
                    }
                }
                receipt.dispatch()?;
                generation = Some(receipt);
                Ok(())
            },
        );
        if let Some(receipt) = generation {
            match &outcome {
                Ok(crate::role_routing::adapters::codex::SidecarOutcome::Result { .. }) => {
                    receipt.complete()?
                }
                Ok(crate::role_routing::adapters::codex::SidecarOutcome::Cancelled) => {
                    receipt.cancel()?
                }
                _ => receipt.fail("codex-request-failed")?,
            }
        }
        outcome
    })
    .await
    .map_err(|error| {
        TurnExecutionFailure::configuration(format!("Role-routing Codex task failed: {error}"))
    })?;
    match outcome {
        Ok(crate::role_routing::adapters::codex::SidecarOutcome::Result { text, usage }) => {
            Ok(CodexStepResult {
                content: text,
                usage,
            })
        }
        Ok(crate::role_routing::adapters::codex::SidecarOutcome::Cancelled) => {
            Err(TurnExecutionFailure::provider(
                ProviderFailureKind::Cancelled,
                "Role-routing Codex actor was cancelled".into(),
            ))
        }
        Ok(crate::role_routing::adapters::codex::SidecarOutcome::Failed(code)) => {
            Err(TurnExecutionFailure::provider(
                ProviderFailureKind::Upstream,
                format!("Role-routing Codex actor failed: {code}"),
            ))
        }
        Err(error) => Err(TurnExecutionFailure::provider(
            ProviderFailureKind::Upstream,
            error,
        )),
    }
}
