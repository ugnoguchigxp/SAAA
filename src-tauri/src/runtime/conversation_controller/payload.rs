//! Bounded reasoning wire projection and exact evidence receipt selection.
use super::*;
use crate::runtime::context::source::Candidate;
use saaa_reasoning_contract::{Budget, Constraints, Context, Message, Request, Role, VERSION};

pub(super) fn prepare(
    state: &AppState,
    input: &StartTurnInput,
    history: &[ConversationMessage],
    context: &ContextManifest<'_>,
) -> Result<(Request, Vec<Candidate>, Vec<Candidate>), String> {
    // World is sent once as evidence, never retained a second time as assistant history.
    let world_free_history = context
        .world
        .map(|world| world.without_world_history(history));
    let mut request = project(input, world_free_history.as_deref().unwrap_or(history))?;
    request.constraints.language = state.sqlite_readers.read(|connection| {
        Ok(crate::persistence::settings::regional_preferences::load(connection)?.language)
    })?;
    if request.constraints.language == "system" {
        request.constraints.language = "auto".into();
    }
    let mut selected = context.selected.to_vec();
    if !context
        .world
        .is_some_and(|world| world.revalidate_current())
    {
        selected.retain(|source| {
            source.source_kind != crate::runtime::context::world::source::WORLD_KIND
        });
    }
    fit_context(&mut request, &selected)?;
    // Slot and byte limits can omit optional sources. Record only evidence actually sent.
    selected.retain(|source| {
        request.context.evidence.iter().any(|evidence| {
            evidence.source
                == format!(
                    "{}:{}@{}",
                    source.source_kind, source.source_id, source.source_version
                )
                && evidence.content == source.content
        })
    });
    if selected
        .iter()
        .any(|source| source.source_kind == crate::runtime::context::world::source::WORLD_KIND)
        && !context
            .world
            .is_some_and(|world| world.revalidate_current())
    {
        request
            .context
            .evidence
            .retain(|evidence| !evidence.source.starts_with("world-model:"));
        selected.retain(|source| {
            source.source_kind != crate::runtime::context::world::source::WORLD_KIND
        });
    }
    let mut omitted = context.omitted.to_vec();
    omitted.extend(
        context
            .selected
            .iter()
            .filter(|source| {
                !selected.iter().any(|sent| {
                    sent.source_kind == source.source_kind && sent.source_id == source.source_id
                })
            })
            .cloned(),
    );
    Ok((request, selected, omitted))
}

pub(super) fn project(
    input: &StartTurnInput,
    history: &[ConversationMessage],
) -> Result<Request, String> {
    let mut messages: Vec<Message> = history
        .iter()
        .filter_map(|m| {
            let role = match m.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return None,
            };
            Some(Message {
                role,
                content: m.content.clone(),
            })
        })
        .collect();
    // Projection's current user message is last. Remove only that occurrence.
    if messages
        .last()
        .is_some_and(|m| matches!(m.role, Role::User) && m.content.trim() == input.content.trim())
    {
        messages.pop();
    }
    let truncated = messages.len() > 32
        || history
            .iter()
            .any(|m| m.role != "user" && m.role != "assistant");
    if messages.len() > 32 {
        messages.drain(..messages.len() - 32);
    }
    let request = Request {
        schema_version: VERSION.into(),
        conversation_id: input.conversation_id.clone(),
        turn_id: input.run_id.clone(),
        request_id: crate::new_id("reasoning"),
        context_revision: REVISION.fetch_add(1, Ordering::Relaxed),
        request: input.content.trim().into(),
        context: Context {
            messages,
            evidence: vec![saaa_reasoning_contract::Evidence {
                world: None,
                id: "host_capabilities".into(), source: "SAAA host capability declaration".into(),
                content: "This voice reasoning route cannot execute coding tools or pi jobs. When asked to implement, continue, inspect or cancel a coding job, state that this route does not support the action and direct the user to the normal text conversation. Never claim to have executed an action.".into(),
            }],
            truncated,
        },
        constraints: Constraints {
            language: "ja".into(),
            local_only: true,
            max_speech_chars: 240,
        },
        budget: Budget {
            timeout_ms: TIMEOUT_MS,
        },
    };
    Ok(request)
}
pub(super) fn fit_context(
    request: &mut Request,
    selected: &[crate::runtime::context::source::Candidate],
) -> Result<(), String> {
    use crate::runtime::context::source::Requirement;

    // `messages` are conversational history, while source snapshots are explicitly marked as
    // evidence. Do not rely on a model inferring a current-state source from the rendered prose.
    // Keep the host capability declaration in slot zero and reserve the remaining bounded slots
    // for required sources first, then the current World frame, then optional context.
    let mut candidates: Vec<_> = selected.iter().collect();
    candidates.sort_by_key(|candidate| match candidate.requirement {
        Requirement::Must => 0_u8,
        _ if candidate.source_kind == crate::runtime::context::world::source::WORLD_KIND => 1,
        Requirement::Should => 2,
        Requirement::May => 3,
    });
    let required_count = candidates
        .iter()
        .filter(|candidate| candidate.requirement == Requirement::Must)
        .count();
    if required_count > 7 {
        return Err("required_context_overflow: reasoning evidence slots exhausted".into());
    }
    for (index, candidate) in candidates.into_iter().take(7).enumerate() {
        request
            .context
            .evidence
            .push(saaa_reasoning_contract::Evidence {
                world: if candidate.source_kind
                    == crate::runtime::context::world::source::WORLD_KIND
                {
                    Some(
                        saaa_reasoning_contract::world::WorldEvidence::from_content(
                            &candidate.content,
                        )
                        .map_err(str::to_string)?,
                    )
                } else {
                    None
                },
                id: format!("source-{index}"),
                source: format!(
                    "{}:{}@{}",
                    candidate.source_kind, candidate.source_id, candidate.source_version
                ),
                content: candidate.content.clone(),
            });
    }
    while request.validate().is_err() || !request.model_input_fits() {
        let Some(index) = request.context.messages.iter().position(|message| {
            !selected
                .iter()
                .filter(|candidate| candidate.requirement == Requirement::Must)
                .any(|candidate| message.content.contains(&candidate.content))
        }) else {
            return Err(
                "required_context_overflow: reasoning input cannot fit without required context"
                    .into(),
            );
        };
        request.context.messages.remove(index);
        request.context.truncated = true;
    }
    request.validate().map_err(str::to_string)?;
    if !request.model_input_fits() {
        return Err("Reasoning input exceeds the model input budget".into());
    }
    Ok(())
}
