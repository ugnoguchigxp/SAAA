use crate::{database_error, validate_identifier, AppState};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppendRunningInput {
    pub(crate) conversation_id: String,
    pub(crate) run_id: String,
    pub(crate) content: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppendRunningInputResult {
    message_id: String,
    transferred: bool,
}

#[tauri::command]
pub(crate) fn append_running_input(
    state: tauri::State<'_, AppState>,
    input: AppendRunningInput,
) -> Result<AppendRunningInputResult, String> {
    if !super::continuation_enabled() {
        return Err("Conversation continuation is disabled".into());
    }
    validate_identifier(&input.conversation_id, "conversation id")?;
    validate_identifier(&input.run_id, "run id")?;
    let content = input.content.trim();
    if content.is_empty() || content.chars().count() > 16_000 {
        return Err("Enter a message".into());
    }
    let message_id = crate::new_id("message");
    let created_at = crate::now_iso();
    let transferred = state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        super::ensure_schema(&transaction).map_err(database_error)?;
        super::commit_visible_message(
            &transaction,
            &input.conversation_id,
            &message_id,
            "user",
            content,
            &created_at,
            None,
            "user_message",
        )
        .map_err(database_error)?;
        let transferred = super::ledger::attach_to_running(
            &transaction,
            &input.conversation_id,
            &input.run_id,
            &message_id,
        )
        .map_err(database_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![created_at, input.conversation_id],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
        Ok(transferred)
    })?;
    Ok(AppendRunningInputResult {
        message_id,
        transferred,
    })
}

#[tauri::command]
pub(crate) fn acknowledge_conversation_message(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    run_id: String,
    message_id: String,
) -> Result<(), String> {
    validate_identifier(&conversation_id, "conversation id")?;
    validate_identifier(&run_id, "run id")?;
    validate_identifier(&message_id, "message id")?;
    state.sqlite_writer.write(|connection| {
        if super::ledger::acknowledge_message_presented(
            connection,
            &conversation_id,
            &run_id,
            &message_id,
        )
        .map_err(database_error)?
        {
            Ok(())
        } else {
            Err("Conversation message was not awaiting presentation".into())
        }
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnconsumedConversationInput {
    message_id: String,
    content: String,
    prior_status: String,
}

#[tauri::command]
pub(crate) fn first_unconsumed_conversation_input(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Option<UnconsumedConversationInput>, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    state.sqlite_readers.read(|connection| {
        super::ledger::first_unconsumed_terminal_input(connection, &conversation_id)
            .map(|item| {
                item.map(
                    |(message_id, content, prior_status)| UnconsumedConversationInput {
                        message_id,
                        content,
                        prior_status,
                    },
                )
            })
            .map_err(database_error)
    })
}

#[tauri::command]
pub(crate) fn conversation_event_head(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<i64, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT COALESCE(MAX(seq),0) FROM conversation_events WHERE conversation_id=?1",
                [&conversation_id],
                |row| row.get(0),
            )
            .map_err(database_error)
    })
}
