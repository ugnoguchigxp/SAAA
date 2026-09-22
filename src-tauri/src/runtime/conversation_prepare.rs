//! Compose after concrete provider setup.
use super::*;
pub(super) struct FreshProviderContext {
    pub(super) envelope: crate::runtime::context::broker::Envelope,
    pub(super) world: Option<crate::runtime::context::world::turn::WorldLive>,
    pub(super) history: Vec<ConversationMessage>,
}

/// Resolves a budget at the concrete provider boundary. OpenAI-compatible tool offers depend on
/// live state, so their exact serialized schema replaces the conservative pre-connect reserve.
pub(super) fn provider_input_budget(
    state: &AppState,
    input: &StartTurnInput,
    session_id: &str,
    provider: &ModelProviderSettings,
) -> Result<crate::runtime::context::broker::ProviderInputBudget, String> {
    let request_options = match provider {
        ModelProviderSettings::OpenAiCompatible(provider) => provider.request_options.as_ref(),
        ModelProviderSettings::DynamicLan(provider) => provider.request_options.as_ref(),
        ModelProviderSettings::AgentSession(_) => {
            let reserve = crate::providers::agent_session::initial_input_reserve(state, input)?;
            return Ok(
                crate::runtime::context::broker::ProviderInputBudget::agent_session()
                    .with_tool_schema_reserve_bytes(reserve),
            );
        }
        ModelProviderSettings::CloudAsr(_)
        | ModelProviderSettings::CloudTts(_)
        | ModelProviderSettings::SystemTts(_) => {
            return Ok(crate::runtime::context::broker::ProviderInputBudget::openai_compatible());
        }
    };
    let tools_enabled = request_options
        .map(|options| options.tools)
        .unwrap_or_else(|| saaa_larm_session::http_api::LlmOptions::standard().tools);
    if !tools_enabled {
        return Ok(
            crate::runtime::context::broker::ProviderInputBudget::openai_compatible()
                .with_tool_schema_reserve_bytes(0),
        );
    }
    let offer = crate::providers::stream::available_agent_tools(
        Some(ProviderOutputPersistence {
            state,
            session_id,
            world: None,
        }),
        input,
        0,
        0,
        0,
    );
    // The chat-completions adapter omits both fields for an empty offer, so reserve the same
    // fragment it actually adds to the wire body rather than a synthetic empty `tools` array.
    let schema_bytes = if offer.definitions.is_empty() {
        0
    } else {
        serde_json::to_vec(&serde_json::json!({
            "tools": offer.definitions,
            "parallel_tool_calls": false,
        }))
        .map_err(|error| format!("could not serialize offered tool schema: {error}"))?
        .len()
    };
    Ok(
        crate::runtime::context::broker::ProviderInputBudget::openai_compatible()
            .with_tool_schema_reserve_bytes(schema_bytes),
    )
}

/// Re-read source-backed context after a provider session has been acquired. This is the context
/// used for the actual wire body; every fallback gets its own single refresh.
pub(super) fn compose_after_connect(
    state: &AppState,
    input: &StartTurnInput,
    identity: &crate::CodexAgentRuntimeSettings,
    regional: &crate::persistence::settings::regional_preferences::RegionalPreferences,
    budget: crate::runtime::context::broker::ProviderInputBudget,
) -> Result<FreshProviderContext, String> {
    let latest = conversation_inputs::load(state, input)?;
    if latest.scope.status != "resolved" {
        return Err("context-scope-changed-after-connect".into());
    }
    if let Some(error) = latest.personal_source_error {
        return Err(error);
    }
    let window = memory::context_window::compose(latest.loaded_context)?;
    let base = budget.apply(window)?;
    let role_candidates = if latest.role_dispatch.is_some() {
        crate::runtime::context::role_projection::project(
            crate::runtime::context::role_projection::RoleProjectionInput {
                allowed_scope_keys: latest
                    .scope
                    .scopes
                    .iter()
                    .map(|scope| scope.key.clone())
                    .collect(),
                initial: latest.personal_candidates,
                amendments: latest.continuation_candidates,
                revoked_source_ids: std::collections::HashSet::new(),
            },
        )?
    } else {
        latest
            .personal_candidates
            .into_iter()
            .chain(latest.continuation_candidates)
            .collect()
    };
    let composed = crate::runtime::context::world::turn::compose_for_app(
        state,
        &input.run_id,
        &latest.scope,
        base,
        role_candidates,
        latest
            .scope
            .scopes
            .iter()
            .map(|scope| scope.key.clone())
            .collect(),
    )?;
    let history = compose_provider_history(
        &input.conversation_id,
        &identity.agent_name,
        &identity.user_name,
        regional,
        &input.input_origin,
        &input.presentation_mode,
        composed.envelope.messages.clone(),
    )?;
    Ok(FreshProviderContext {
        envelope: composed.envelope,
        world: composed.world,
        history,
    })
}

/// The World-free rendering of a composed history. `None` when the history carries no World block,
/// so the caller can reuse the original borrow without cloning.
pub(super) fn world_free_history(
    history: &[ConversationMessage],
    world: Option<&crate::runtime::context::world::turn::WorldLive>,
) -> Option<Vec<ConversationMessage>> {
    let blocks = world.and_then(|world| world.blocks())?;
    Some(
        history
            .iter()
            .filter_map(|message| {
                if message.role == "assistant" && message.content == blocks.with_world {
                    blocks
                        .without_world
                        .clone()
                        .map(|content| ConversationMessage {
                            content,
                            ..message.clone()
                        })
                } else {
                    Some(message.clone())
                }
            })
            .collect(),
    )
}
