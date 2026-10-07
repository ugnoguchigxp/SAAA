use super::*;
use saaa_personal_state_core::world::runtime_frame::FrameValidity;
pub(super) fn validate_result(context: &QueueContext, state: &AppState) -> Result<(), String> {
    let (current, scope) = project_window(state, &context.run_id, &context.message_id)?;
    let personal = state.sqlite_readers.read(|c| {
        memory::personal_state::sources::episode_export::validate_run(c, &context.run_id)?;
        personal_state(c, &scope, &context.message_id)
    })?;
    if current.messages != context.source_messages
        || scope != context.scope
        || personal.0 != context.personal_memory
        || personal.1 != context.personal_stamp
    {
        return Err("メモリーまたは会話の根拠が応答中に変化しました。再実行してください。".into());
    }
    if let Some((service, frame)) = &context.world {
        match service.validate_result(frame) {
            Ok(FrameValidity::Current) => {}
            _ => return Err("WorldModelの根拠が応答中に変化しました。再実行してください。".into()),
        }
    }
    Ok(())
}

pub(super) fn validate_commit(
    context: &QueueContext,
    connection: &rusqlite::Connection,
) -> Result<(), String> {
    let (current, scope) =
        project_window_connection(connection, &context.run_id, &context.message_id)?;
    memory::personal_state::sources::episode_export::validate_run(connection, &context.run_id)?;
    let personal = personal_state(connection, &scope, &context.message_id)?;
    if current.messages != context.source_messages
        || scope != context.scope
        || personal.0 != context.personal_memory
        || personal.1 != context.personal_stamp
    {
        return Err("メモリーまたは会話の根拠が保存前に変化しました。".into());
    }
    if let Some((service, frame)) = &context.world {
        service
            .validate_db_result(connection, frame)
            .map_err(|error| error.code().to_string())?;
    }
    memory::personal_state::sources::episode_export::capture_snapshot_inputs(
        connection,
        &context.run_id,
        &context.personal_inputs,
    )?;
    memory::personal_state::sources::episode_export::reuse_history(
        connection,
        &context.run_id,
        &scope,
        &context.message_id,
    )?;
    Ok(())
}
