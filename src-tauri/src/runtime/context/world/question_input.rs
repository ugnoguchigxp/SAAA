//! C2: resolve the graph question from the source-of-record current input.
//!
//! The only text the parser may see is the saved current input that `runtime_runs.input_message_id`
//! points at: the same conversation, a running run, and a user/transcript row. Assistant history,
//! tool results and recall bodies are never parsed. A missing or unavailable source-of-record is
//! `NotRequested` (fail closed), never an error that blocks the turn.
use super::question::{parse_graph_question, QuestionParse};
use crate::{database_error, AppState};
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) fn read(state: &AppState, run_id: &str) -> QuestionParse {
    match state
        .sqlite_readers
        .read(|connection| read_text(connection, run_id))
    {
        Ok(Some(text)) => parse_graph_question(&text),
        _ => QuestionParse::NotRequested,
    }
}

/// The source-of-record check: a running run whose saved current input still exists in the same
/// conversation with a user/transcript role.
pub(crate) fn read_text(connection: &Connection, run_id: &str) -> Result<Option<String>, String> {
    let run = connection
        .query_row(
            "SELECT conversation_id, input_message_id FROM runtime_runs
              WHERE id = ?1 AND status = 'running'",
            params![run_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    let Some((conversation_id, input_message_id)) = run else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT content FROM conversation_messages
              WHERE id = ?1 AND conversation_id = ?2 AND role IN ('user', 'transcript')",
            params![input_message_id, conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_runs(id TEXT PRIMARY KEY, conversation_id TEXT, status TEXT, input_message_id TEXT);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT, content TEXT);
                 INSERT INTO conversations(id) VALUES('c-1');",
            )
            .ok();
        connection
    }

    fn seed(connection: &Connection, run: &str, status: &str, message: &str, role: &str, content: &str) {
        connection
            .execute(
                "INSERT INTO runtime_runs(id, conversation_id, status, input_message_id) VALUES(?1, 'c-1', ?2, ?3)",
                params![run, status, message],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_messages(id, conversation_id, role, content) VALUES(?1, 'c-1', ?2, ?3)",
                params![message, role, content],
            )
            .unwrap();
    }

    #[test]
    fn world_g1_02_reads_only_the_running_source_of_record() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_runs(id TEXT PRIMARY KEY, conversation_id TEXT, status TEXT, input_message_id TEXT);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT, content TEXT);
                 INSERT INTO runtime_runs VALUES('run', 'c-1', 'running', 'input');
                 INSERT INTO conversation_messages VALUES('input', 'c-1', 'user', '「tech」は何に影響しますか？');
                 INSERT INTO conversation_messages VALUES('assistant', 'c-1', 'assistant', '「tech」は何に影響しますか？');",
            )
            .unwrap();
        let text = read_text(&connection, "run").unwrap();
        assert_eq!(text.as_deref(), Some("「tech」は何に影響しますか？"));
        assert!(parse_graph_question(&text.unwrap()).is_requested());
    }

    #[test]
    fn world_g1_02_rejects_other_run_role_and_missing_run() {
        let connection = empty_connection();
        seed(&connection, "run", "completed", "input", "user", "「tech」は何に影響しますか？");
        assert_eq!(read_text(&connection, "run").unwrap(), None);

        let connection = empty_connection();
        seed(&connection, "run", "running", "input", "assistant", "「tech」は何に影響しますか？");
        assert_eq!(read_text(&connection, "run").unwrap(), None);

        let connection = empty_connection();
        seed(&connection, "run", "running", "input", "user", "「tech」は何に影響しますか？");
        assert_eq!(read_text(&connection, "missing").unwrap(), None);
    }

    #[test]
    fn world_g1_02_run_message_mismatch_is_rejected() {
        let connection = empty_connection();
        connection
            .execute_batch(
                "INSERT INTO runtime_runs VALUES('run', 'c-1', 'running', 'input');
                 INSERT INTO conversation_messages VALUES('input', 'c-2', 'user', '「tech」は何に影響しますか？');",
            )
            .unwrap();
        assert_eq!(read_text(&connection, "run").unwrap(), None);
    }
}
