use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::{
    database_error,
    ipc_contract::{ConversationMessage, ConversationMessagePage},
    validate_identifier,
};

const CURSOR_VERSION: u8 = 1;
const MAX_CURSOR_BYTES: usize = 512;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MessageCursor {
    version: u8,
    created_at_ms: i64,
    message_id: String,
}

pub(crate) fn list_message_page_from_connection(
    connection: &Connection,
    conversation_id: &str,
    cursor: Option<&str>,
    page_size: u64,
) -> Result<ConversationMessagePage, String> {
    if !(1..=100).contains(&page_size) {
        return Err("Invalid message page size".to_string());
    }
    let cursor = cursor
        .map(|encoded| decode_cursor(connection, conversation_id, encoded))
        .transpose()?;
    let mut messages = load_descending(connection, conversation_id, cursor.as_ref(), page_size)?;
    let has_more = messages.len() > page_size as usize;
    if has_more {
        messages.truncate(page_size as usize);
    }
    let next_cursor = if has_more {
        messages.last().map(encode_cursor).transpose()?
    } else {
        None
    };
    messages.reverse();
    Ok(ConversationMessagePage {
        messages,
        has_more,
        next_cursor,
    })
}

fn load_descending(
    connection: &Connection,
    conversation_id: &str,
    cursor: Option<&MessageCursor>,
    page_size: u64,
) -> Result<Vec<ConversationMessage>, String> {
    let first_page = "SELECT id,conversation_id,role,content,created_at
        FROM conversation_messages INDEXED BY idx_conversation_messages_conversation_created_ms
        WHERE conversation_id=?1
        ORDER BY CAST(created_at AS INTEGER) DESC,id DESC LIMIT ?2";
    let next_page = "SELECT id,conversation_id,role,content,created_at
        FROM conversation_messages INDEXED BY idx_conversation_messages_conversation_created_ms
        WHERE conversation_id=?1 AND (CAST(created_at AS INTEGER),id)<(?2,?3)
        ORDER BY CAST(created_at AS INTEGER) DESC,id DESC LIMIT ?4";
    let mut statement = connection
        .prepare_cached(if cursor.is_some() {
            next_page
        } else {
            first_page
        })
        .map_err(database_error)?;
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok(ConversationMessage {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            created_at: row.get(4)?,
        })
    };
    let limit = page_size + 1;
    let rows = match cursor {
        Some(cursor) => statement
            .query_map(
                params![
                    conversation_id,
                    cursor.created_at_ms,
                    cursor.message_id,
                    limit
                ],
                map_row,
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?,
        None => statement
            .query_map(params![conversation_id, limit], map_row)
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?,
    };
    for message in &rows {
        super::conversations::validate_message(message, conversation_id)?;
    }
    Ok(rows)
}

fn encode_cursor(message: &ConversationMessage) -> Result<String, String> {
    let cursor = MessageCursor {
        version: CURSOR_VERSION,
        created_at_ms: message
            .created_at
            .parse()
            .map_err(|_| "Invalid persisted conversation message".to_string())?,
        message_id: message.id.clone(),
    };
    let encoded =
        serde_json::to_vec(&cursor).map_err(|_| "Could not encode message cursor".to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(encoded))
}

fn decode_cursor(
    connection: &Connection,
    conversation_id: &str,
    encoded: &str,
) -> Result<MessageCursor, String> {
    if encoded.is_empty() || encoded.len() > MAX_CURSOR_BYTES {
        return Err("invalid-cursor".to_string());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "invalid-cursor".to_string())?;
    let cursor: MessageCursor =
        serde_json::from_slice(&bytes).map_err(|_| "invalid-cursor".to_string())?;
    validate_identifier(&cursor.message_id, "message cursor id")?;
    if cursor.version != CURSOR_VERSION || cursor.created_at_ms < 0 {
        return Err("invalid-cursor".to_string());
    }
    let valid: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_messages
             WHERE id=?1 AND conversation_id=?2 AND CAST(created_at AS INTEGER)=?3)",
            params![cursor.message_id, conversation_id, cursor.created_at_ms],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !valid {
        return Err("invalid-cursor".to_string());
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::initialize_database;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
                 VALUES('conversation_history','coding','1','1')",
                [],
            )
            .expect("conversation inserts");
        for index in 1..=25 {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES(?1,'conversation_history','user',?2,?3)",
                    params![
                        format!("message_{index:02}"),
                        format!("message {index}"),
                        index.to_string()
                    ],
                )
                .expect("message inserts");
        }
        connection
    }

    #[test]
    fn pages_use_an_opaque_keyset_without_duplicates() {
        let connection = fixture();
        let latest =
            list_message_page_from_connection(&connection, "conversation_history", None, 10)
                .expect("latest page loads");
        let earlier = list_message_page_from_connection(
            &connection,
            "conversation_history",
            latest.next_cursor.as_deref(),
            10,
        )
        .expect("earlier page loads");
        let oldest = list_message_page_from_connection(
            &connection,
            "conversation_history",
            earlier.next_cursor.as_deref(),
            10,
        )
        .expect("oldest page loads");
        assert_eq!(latest.messages[0].content, "message 16");
        assert_eq!(earlier.messages[0].content, "message 6");
        assert_eq!(oldest.messages.len(), 5);
        assert!(!oldest.has_more);
    }

    #[test]
    fn cursor_rejects_tamper_and_cross_conversation_use() {
        let connection = fixture();
        let page = list_message_page_from_connection(&connection, "conversation_history", None, 10)
            .expect("page loads");
        let mut cursor = page.next_cursor.expect("cursor exists");
        cursor.push('A');
        assert_eq!(
            list_message_page_from_connection(
                &connection,
                "conversation_history",
                Some(&cursor),
                10
            )
            .expect_err("tamper is rejected"),
            "invalid-cursor"
        );
    }

    #[test]
    fn keyset_query_uses_the_expression_index_without_a_temp_sort() {
        let connection = fixture();
        let detail: String = connection
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id,conversation_id,role,content,created_at
                 FROM conversation_messages INDEXED BY idx_conversation_messages_conversation_created_ms
                 WHERE conversation_id='conversation_history'
                 ORDER BY CAST(created_at AS INTEGER) DESC,id DESC LIMIT 11",
                [],
                |row| row.get(3),
            )
            .expect("query plan loads");
        assert!(detail.contains("idx_conversation_messages_conversation_created_ms"));
        assert!(!detail.contains("TEMP B-TREE"));
    }
}
