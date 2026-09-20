//! Session cleanup helpers. Kept out of the server wiring module so the module-size ratchet stays
//! bounded and the server module only decides when cleanup runs.

use rusqlite::params;

use super::sessions::Session;
use crate::persistence::SqliteWriter;

/// Deletes a never-ready session's conversation row when it has no messages and no decisions. A
/// session with any history is kept, matching the existing conversation retention policy.
pub fn delete_empty_conversation(writer: &SqliteWriter, session: &Session) {
    let conversation_id = session.conversation_id().to_string();
    let _ = writer.write(move |connection| {
        connection
            .execute(
                "DELETE FROM conversations
                  WHERE id = ?1
                    AND NOT EXISTS (SELECT 1 FROM conversation_messages WHERE conversation_id = ?1)
                    AND NOT EXISTS (SELECT 1 FROM tool_selection_decisions WHERE conversation_id = ?1)",
                params![conversation_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    });
}
