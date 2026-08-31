use rusqlite::{params, TransactionBehavior};

use super::conversations::validate_conversation_write_target;
use super::effective_route::effective_route_snapshot;
use super::{
    ensure_primary_conversation, list_conversations_from_connection,
    save_settings_documents_to_connection, validate_settings_batch, validate_settings_document,
};
use crate::ipc_contract::ConversationMessage;
use crate::{
    database_error, new_id, now_iso, spawn_situation_monitor, validate_identifier, AppSnapshot,
    AppState, AppendMessageInput, Conversation, CreateConversationInput, LarmRuntimeStatus,
    SaveSettingsDocumentsInput, SettingsDocument,
};

pub(crate) fn get_app_snapshot(state: &AppState) -> Result<AppSnapshot, String> {
    let provider_probes = state
        .provider_probes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    state
        .voice_profile
        .read_with_snapshot(&state.sqlite_readers, |connection, voice_profile| {
            let conversations = list_conversations_from_connection(connection)?;
            let primary_conversation_id = conversations
                .iter()
                .find(|conversation| conversation.id == crate::PRIMARY_CONVERSATION_ID)
                .map(|conversation| conversation.id.clone())
                .ok_or_else(|| "Primary conversation is unavailable".to_string())?;
            Ok(AppSnapshot {
                settings: state.sqlite_readers.settings_snapshot(connection)?,
                conversations,
                primary_conversation_id,
                effective_route: effective_route_snapshot(connection, &provider_probes)?,
                larm_runtime: LarmRuntimeStatus {
                    state: state.larm_gate.state(),
                    message: state.larm_gate.public_message(),
                    contract_commit: crate::providers::larm::CONTRACT_COMMIT,
                },
                voice_profile,
            })
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
        &state.sqlite_writer,
        situation_settings,
        |connection| save_settings_documents_to_connection(connection, &input.documents),
    )?;
    state
        .provider_probes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    crate::providers::service_harness::clear_cache();
    if enabled {
        spawn_situation_monitor(state.sqlite_writer.clone(), state.situation.clone());
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
    state.sqlite_writer.write(|connection| {
        if input.task_mode == "conversation" {
            let conversation = ensure_primary_conversation(connection)?;
            crate::voice_behavior::ensure_policy(connection, &conversation.id)?;
            return Ok(conversation);
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
    })
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
    state.sqlite_writer.write_transaction(
        TransactionBehavior::Deferred,
        |transaction| {
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
            Ok(())
        },
    )?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_creation_keeps_voice_policy_scoped_to_normal_chat() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);

        let coding = create_conversation(
            &state,
            CreateConversationInput {
                title: Some("Coding".to_string()),
                task_mode: "coding".to_string(),
            },
        )
        .expect("coding conversation creates");
        let normal = create_conversation(
            &state,
            CreateConversationInput {
                title: None,
                task_mode: "conversation".to_string(),
            },
        )
        .expect("normal conversation loads");

        state
            .sqlite_writer
            .read_serialized(|connection| {
                let coding_policy: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM conversation_voice_policies WHERE conversation_id=?1",
                        [&coding.id],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                let normal_policy: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM conversation_voice_policies WHERE conversation_id=?1",
                        [&normal.id],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                assert_eq!(coding_policy, 0);
                assert_eq!(normal_policy, 1);
                Ok(())
            })
            .expect("voice policies inspect");
    }
}
