use rusqlite::{params, Connection};

use crate::ipc_contract::ConversationMessage;
use crate::{
    database_error, memory, now_iso, validate_identifier, Conversation, PRIMARY_CONVERSATION_ID,
    PRIMARY_CONVERSATION_TITLE,
};

#[cfg(test)]
pub(crate) fn list_messages_from_connection(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ConversationMessage>, String> {
    let mut statement = connection
        .prepare_cached(
            "SELECT id, conversation_id, role, content, created_at
             FROM (
               SELECT rowid AS ordinal, id, conversation_id, role, content, created_at
               FROM conversation_messages
               WHERE conversation_id = ?1
               ORDER BY CAST(created_at AS INTEGER) DESC, rowid DESC
               LIMIT 100
             )
             ORDER BY CAST(created_at AS INTEGER) ASC, ordinal ASC",
        )
        .map_err(database_error)?;
    let messages = statement
        .query_map(params![conversation_id], |row| {
            Ok(ConversationMessage {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for message in &messages {
        validate_message(message, conversation_id)?;
    }
    Ok(messages)
}

pub(super) fn validate_message(
    message: &ConversationMessage,
    conversation_id: &str,
) -> Result<(), String> {
    validate_identifier(&message.id, "message id")?;
    validate_identifier(&message.conversation_id, "conversation id")?;
    if message.conversation_id != conversation_id
        || !matches!(
            message.role.as_str(),
            "user" | "assistant" | "system" | "transcript"
        )
        || message.content.is_empty()
        || message.content.chars().count()
            > if message.role == "assistant" {
                64_000
            } else {
                16_000
            }
        || message.created_at.parse::<u128>().is_err()
    {
        return Err("Invalid persisted conversation message".to_string());
    }
    Ok(())
}

pub(crate) fn ensure_primary_conversation(connection: &Connection) -> Result<Conversation, String> {
    let now = now_iso();
    connection
        .execute(
            "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES (?1, ?2, 'conversation', ?3, ?3)",
            params![PRIMARY_CONVERSATION_ID, PRIMARY_CONVERSATION_TITLE, now],
        )
        .map_err(database_error)?;
    let conversation = connection
        .query_row(
            "SELECT id, title, task_mode, created_at, updated_at
             FROM conversations WHERE id = ?1",
            params![PRIMARY_CONVERSATION_ID],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    task_mode: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .map_err(database_error)?;
    if conversation.task_mode != "conversation" {
        return Err("Primary conversation has an invalid task mode".to_string());
    }
    memory::control_plane::ensure_continuity_state(connection, PRIMARY_CONVERSATION_ID, &now)
        .map_err(database_error)?;
    Ok(conversation)
}

pub(crate) fn validate_conversation_write_target(
    conversation_id: &str,
    task_mode: &str,
) -> Result<(), String> {
    if task_mode == "conversation" && conversation_id != PRIMARY_CONVERSATION_ID {
        return Err("Normal conversation writes must use the primary conversation".to_string());
    }
    Ok(())
}

pub(crate) fn list_conversations_from_connection(
    connection: &Connection,
) -> Result<Vec<Conversation>, String> {
    let mut statement = connection
        .prepare_cached(
            "SELECT id, title, task_mode, created_at, updated_at
             FROM conversations
             WHERE task_mode = 'coding' OR id = ?1
             ORDER BY updated_at DESC LIMIT 30",
        )
        .map_err(database_error)?;
    let conversations = statement
        .query_map(params![PRIMARY_CONVERSATION_ID], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                task_mode: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for conversation in &conversations {
        validate_identifier(&conversation.id, "conversation id")?;
        if !matches!(conversation.task_mode.as_str(), "conversation" | "coding")
            || conversation
                .title
                .as_ref()
                .is_some_and(|title| title.chars().count() > 120)
            || conversation.created_at.parse::<u128>().is_err()
            || conversation.updated_at.parse::<u128>().is_err()
        {
            return Err("Invalid persisted conversation".to_string());
        }
    }
    Ok(conversations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{initialize_database, PRIMARY_CONVERSATION_ID, PRIMARY_CONVERSATION_TITLE};
    use rusqlite::{params, Connection};

    #[test]
    fn conversation_context_keeps_the_latest_hundred_messages_in_order() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, task_mode, created_at, updated_at)
             VALUES ('conversation_history', 'conversation', '0', '0')",
                [],
            )
            .expect("conversation inserts");
        for index in 0..105 {
            connection
            .execute(
                "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                 VALUES (?1, 'conversation_history', 'user', ?2, ?3)",
                params![format!("message_{index}"), index.to_string(), index.to_string()],
            )
            .expect("message inserts");
        }
        let messages = list_messages_from_connection(&connection, "conversation_history")
            .expect("messages load");
        assert_eq!(messages.len(), 100);
        assert_eq!(messages.first().expect("first message").content, "5");
        assert_eq!(messages.last().expect("last message").content, "104");
    }

    #[test]
    fn primary_conversation_is_idempotent_and_preserves_legacy_history() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let initialized_primary_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = ?1",
                params![PRIMARY_CONVERSATION_ID],
                |row| row.get(0),
            )
            .expect("initialized primary count loads");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES ('legacy-conversation', 'Legacy', 'conversation', '0', '0')",
                [],
            )
            .expect("legacy conversation inserts");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES ('coding-conversation', 'Coding', 'coding', '0', '0')",
                [],
            )
            .expect("coding conversation inserts");

        let first = ensure_primary_conversation(&connection).expect("primary creates");
        let second = ensure_primary_conversation(&connection).expect("primary reuses");
        let primary_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = ?1",
                params![PRIMARY_CONVERSATION_ID],
                |row| row.get(0),
            )
            .expect("primary count loads");
        let legacy_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = 'legacy-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("legacy count loads");
        let visible =
            list_conversations_from_connection(&connection).expect("visible conversations load");

        assert_eq!(first.id, PRIMARY_CONVERSATION_ID);
        assert_eq!(first.title.as_deref(), Some(PRIMARY_CONVERSATION_TITLE));
        assert_eq!(first.id, second.id);
        assert_eq!(primary_count, 1);
        assert_eq!(initialized_primary_count, 1);
        assert_eq!(legacy_count, 1);
        assert!(visible
            .iter()
            .any(|conversation| conversation.id == PRIMARY_CONVERSATION_ID));
        assert!(visible
            .iter()
            .any(|conversation| conversation.id == "coding-conversation"));
        assert!(!visible
            .iter()
            .any(|conversation| conversation.id == "legacy-conversation"));
    }

    #[test]
    fn conversation_write_scope_allows_only_primary_normal_chat_and_any_coding_thread() {
        assert!(
            validate_conversation_write_target(PRIMARY_CONVERSATION_ID, "conversation").is_ok()
        );
        assert!(validate_conversation_write_target("coding-thread", "coding").is_ok());
        assert!(
            validate_conversation_write_target("legacy-conversation", "conversation")
                .expect_err("legacy normal write is rejected")
                .contains("primary conversation")
        );
    }
}
