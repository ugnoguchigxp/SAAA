use rusqlite::params;

use super::effective_route::effective_route_snapshot;
use super::{
    ensure_primary_conversation, list_conversations_from_connection, list_settings_documents,
    save_settings_documents_to_connection, validate_conversation_write_target,
    validate_settings_batch, validate_settings_document,
};
use crate::ipc_contract::ConversationMessage;
use crate::{
    database_error, new_id, now_iso, spawn_situation_monitor, validate_identifier, AppSnapshot,
    AppState, AppendMessageInput, Conversation, CreateConversationInput, LarmRuntimeStatus,
    SaveSettingsDocumentsInput, SettingsDocument,
};

pub(crate) fn get_app_snapshot(state: &AppState) -> Result<AppSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let primary_conversation = ensure_primary_conversation(&connection)?;
    let mut conversations = list_conversations_from_connection(&connection)?;
    if !conversations
        .iter()
        .any(|conversation| conversation.id == primary_conversation.id)
    {
        conversations.push(primary_conversation.clone());
    }
    Ok(AppSnapshot {
        settings: list_settings_documents(&connection)?,
        conversations,
        primary_conversation_id: primary_conversation.id,
        effective_route: effective_route_snapshot(
            &connection,
            &state
                .provider_probes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )?,
        larm_runtime: LarmRuntimeStatus {
            state: state.larm_gate.state(),
            message: state.larm_gate.public_message(),
            contract_commit: crate::providers::larm::CONTRACT_COMMIT,
        },
        voice_profile: state.voice_profile.snapshot(&connection)?,
    })
}

pub(crate) fn save_settings_documents(
    state: &AppState,
    input: SaveSettingsDocumentsInput,
) -> Result<Vec<SettingsDocument>, String> {
    for document in &input.documents {
        validate_settings_document(document)?;
    }
    validate_settings_batch(&input.documents)?;
    let situation_settings = input
        .documents
        .iter()
        .find(|document| document.namespace == "situation.runtime" && document.key == "default")
        .ok_or_else(|| "Situation settings are required".to_string())
        .and_then(|document| {
            serde_json::from_value::<crate::situation::contracts::SituationRuntimeSettings>(
                document.value_json.clone(),
            )
            .map_err(|error| format!("Invalid Situation settings: {error}"))
        })?;
    let enabled = situation_settings.enabled;
    let saved = state.situation.configure_and_persist(
        &state.connection,
        situation_settings,
        |connection| save_settings_documents_to_connection(connection, &input.documents),
    )?;
    state
        .provider_probes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    if enabled {
        spawn_situation_monitor(state.connection.clone(), state.situation.clone());
    }
    Ok(saved)
}

pub(crate) fn create_conversation(
    state: &AppState,
    input: CreateConversationInput,
) -> Result<Conversation, String> {
    if !matches!(input.task_mode.as_str(), "conversation" | "coding") {
        return Err("Unsupported task mode".to_string());
    }
    let title = input
        .title
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty());
    if title
        .as_ref()
        .is_some_and(|title| title.chars().count() > 120)
    {
        return Err("Conversation title exceeds the 120 character limit".to_string());
    }
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    if input.task_mode == "conversation" {
        return ensure_primary_conversation(&connection);
    }
    let now = now_iso();
    let conversation = Conversation {
        id: new_id("conversation"),
        title,
        task_mode: input.task_mode,
        created_at: now.clone(),
        updated_at: now,
    };
    connection
        .execute(
            "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                conversation.id,
                conversation.title,
                conversation.task_mode,
                conversation.created_at,
                conversation.updated_at,
            ],
        )
        .map_err(database_error)?;
    Ok(conversation)
}

pub(crate) fn append_message(
    state: &AppState,
    input: AppendMessageInput,
) -> Result<ConversationMessage, String> {
    validate_identifier(&input.conversation_id, "conversation id")?;
    let content = input.content.trim();
    if content.is_empty() {
        return Err("Message cannot be empty".to_string());
    }
    if content.chars().count() > 16_000 {
        return Err("Message exceeds the 16,000 character limit".to_string());
    }
    if !matches!(
        input.role.as_str(),
        "user" | "assistant" | "system" | "transcript"
    ) {
        return Err("Unsupported message role".to_string());
    }

    let message = ConversationMessage {
        id: new_id("message"),
        conversation_id: input.conversation_id,
        role: input.role,
        content: content.to_string(),
        created_at: now_iso(),
    };
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    let task_mode: String = transaction
        .query_row(
            "SELECT task_mode FROM conversations WHERE id = ?1",
            params![message.conversation_id],
            |row| row.get(0),
        )
        .map_err(|_| "Conversation does not exist".to_string())?;
    validate_conversation_write_target(&message.conversation_id, &task_mode)?;
    transaction
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id,
                message.conversation_id,
                message.role,
                message.content,
                message.created_at,
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![message.created_at, message.conversation_id],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(message)
}
