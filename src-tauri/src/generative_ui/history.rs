use crate::{database_error, ipc_contract::ConversationMessage};
use rusqlite::{params, Connection};
use serde::Serialize;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageWindow {
    messages: Vec<ConversationMessage>,
    has_more: bool,
    next_cursor: Option<String>,
    has_newer: bool,
    newer_cursor: Option<String>,
}
/// Cursors are existing message identifiers, resolved and scoped on the server.
/// The original opaque-cursor API is kept intact for other callers.
pub(crate) fn load_window(
    c: &Connection,
    conversation: &str,
    cursor: Option<&str>,
    direction: &str,
) -> Result<MessageWindow, String> {
    if !["before", "after"].contains(&direction) {
        return Err("Invalid history direction".into());
    }
    let anchor = cursor.map(|id| c.query_row("SELECT CAST(created_at AS INTEGER) FROM conversation_messages WHERE id=?1 AND conversation_id=?2", params![id,conversation],|r|r.get::<_,i64>(0)).map_err(|_|"Invalid history cursor".to_string())).transpose()?;
    let (comparison, order) = if direction == "after" {
        (">", "ASC")
    } else {
        ("<", "DESC")
    };
    let sql = if anchor.is_some() {
        format!("SELECT id,conversation_id,role,content,created_at FROM conversation_messages WHERE conversation_id=?1 AND (CAST(created_at AS INTEGER),id){comparison}(?2,?3) ORDER BY CAST(created_at AS INTEGER) {order},id {order} LIMIT 30")
    } else {
        "SELECT id,conversation_id,role,content,created_at FROM conversation_messages WHERE conversation_id=?1 ORDER BY CAST(created_at AS INTEGER) DESC,id DESC LIMIT 30".into()
    };
    let mut stmt = c.prepare(&sql).map_err(database_error)?;
    let row = |r: &rusqlite::Row<'_>| {
        Ok(ConversationMessage {
            id: r.get(0)?,
            conversation_id: r.get(1)?,
            role: r.get(2)?,
            content: r.get(3)?,
            created_at: r.get(4)?,
            parts: None,
        })
    };
    let mut messages = if let Some(time) = anchor {
        stmt.query_map(params![conversation, time, cursor], row)
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
    } else {
        stmt.query_map([conversation], row)
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
    }
    .map_err(database_error)?;
    if direction == "before" || anchor.is_none() {
        messages.reverse();
    }
    super::store::hydrate(c, &mut messages)?;
    let exists = |message: Option<&ConversationMessage>, op: &str| -> Result<bool, String> {
        let Some(message) = message else {
            return Ok(false);
        };
        c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE conversation_id=?1 AND (CAST(created_at AS INTEGER),id){op}(?2,?3))"),params![conversation,message.created_at.parse::<i64>().map_err(|_|"Invalid message timestamp")?,message.id],|r|r.get(0)).map_err(database_error)
    };
    let has_more = exists(messages.first(), "<")?;
    let has_newer = exists(messages.last(), ">")?;
    Ok(MessageWindow {
        next_cursor: messages.first().map(|m| m.id.clone()),
        newer_cursor: messages.last().map(|m| m.id.clone()),
        messages,
        has_more,
        has_newer,
    })
}
#[tauri::command]
pub(crate) async fn list_message_window(
    state: tauri::State<'_, crate::AppState>,
    conversation_id: String,
    cursor: Option<String>,
    direction: String,
) -> Result<MessageWindow, String> {
    crate::validate_identifier(&conversation_id, "conversation id")?;
    state
        .sqlite_readers
        .read_async(move |c| load_window(c, &conversation_id, cursor.as_deref(), &direction))
        .await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generative_ui_history_keyset_handles_100000_rows_and_equal_timestamps() {
        let mut c = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&c).unwrap();
        let tx = c.transaction().unwrap();
        {
            let mut s = tx
                .prepare("INSERT INTO conversation_messages VALUES(?1,?2,'user','text',?3)")
                .unwrap();
            for n in 0..100_000 {
                s.execute(params![
                    format!("m{n:06}"),
                    crate::PRIMARY_CONVERSATION_ID,
                    (n / 2).to_string()
                ])
                .unwrap();
            }
        }
        tx.commit().unwrap();
        let latest = load_window(&c, crate::PRIMARY_CONVERSATION_ID, None, "before").unwrap();
        assert_eq!(latest.messages.len(), 30);
        assert!(!latest.has_newer);
        assert!(latest.has_more);
        let older = load_window(
            &c,
            crate::PRIMARY_CONVERSATION_ID,
            latest.next_cursor.as_deref(),
            "before",
        )
        .unwrap();
        let newer = load_window(
            &c,
            crate::PRIMARY_CONVERSATION_ID,
            older.newer_cursor.as_deref(),
            "after",
        )
        .unwrap();
        assert_eq!(
            newer.messages.iter().map(|m| &m.id).collect::<Vec<_>>(),
            latest.messages.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
        assert!(load_window(&c, "other", latest.next_cursor.as_deref(), "before").is_err());
    }
}
