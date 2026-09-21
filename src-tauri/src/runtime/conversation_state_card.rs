use super::*;

pub(crate) fn persist_card(
    state: &AppState,
    input: &StartTurnInput,
    events: &dyn RuntimeEventSender,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let card = crate::runtime::context::world::host_answer::prepare_card(
        state,
        &input.run_id,
        &input.content,
    );
    events.set_completion_speech(&input.run_id, card.text.clone());
    persist_conversation_success_with_state(state, input, &card.text, |connection, _| {
        if let Some((service, frame)) = &card.world {
            service
                .validate_db_result(connection, frame)
                .map_err(|error| error.code().to_string())?;
        }
        Ok(())
    })
    .map_err(Into::into)
}
