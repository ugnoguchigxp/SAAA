use crate::database_error;
/// Check persisted output permission before invoking external event callbacks.
/// Never hold the database writer across a callback. The UI rejects queued
/// events for revoked run IDs after the forget notification.
pub(crate) fn send_completed(
    state: &crate::AppState,
    input: &crate::StartTurnInput,
    on_event: &dyn crate::runtime::event_hub::RuntimeEventSender,
    message: &crate::ipc_contract::ConversationMessage,
) -> Result<(), String> {
    use crate::ipc_contract::RuntimeEvent;
    let (presentation, voice_policy) =
        crate::voice_behavior::completion_state(state, &input.run_id, &input.conversation_id);
    state.sqlite_writer.read_serialized(|c| {
        crate::memory::personal_state::generation::allow_run(c, &input.run_id)?;
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1)",
                [&message.id],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if !exists {
            return Err("personal-output-invalidated".into());
        }
        Ok(())
    })?;
    let _ = on_event.send(RuntimeEvent::MessageCompleted {
        run_id: input.run_id.clone(),
        message: message.clone(),
        presentation,
        voice_policy,
    });
    Ok(())
}
