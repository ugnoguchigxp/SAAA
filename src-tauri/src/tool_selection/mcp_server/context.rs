//! Host scope for an MCP session.
//!
//! The principal comes from the local profile; the project comes only from the trusted server
//! configuration. Neither `clientInfo`, the HTTP body nor tool arguments can widen scope. A
//! session links to a real conversation row so decisions satisfy the ledger foreign key without
//! fabricating a user message.

use rusqlite::params;

use super::sessions::Session;
use crate::persistence::SqliteWriter;
use crate::tool_selection::RequestContext;

/// Creates the host conversation row for a session, or leaves an existing row untouched.
pub fn create_conversation(writer: &SqliteWriter, conversation_id: &str) -> Result<(), String> {
    let conversation_id = conversation_id.to_string();
    let now = crate::now_iso();
    writer.write(move |connection| {
        connection
            .execute(
                "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES (?1, 'MCP session', 'conversation', ?2, ?2)",
                params![conversation_id, now],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    })
}

/// True when a project id from the trusted configuration exists on this host.
pub fn project_exists(writer: &SqliteWriter, project_id: &str) -> bool {
    let project_id = project_id.to_string();
    writer
        .read_serialized(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM coding_workspaces WHERE id = ?1)",
                    params![project_id],
                    |row| row.get::<_, i64>(0),
                )
                .map(|value| value == 1)
                .map_err(|error| error.to_string())
        })
        .unwrap_or(false)
}

/// Builds the fixed request context for one session call.
pub fn session_context(session: &Session) -> RequestContext {
    RequestContext::new(session.principal_id(), session.conversation_id())
        .with_run(Some(session.run_id().to_string()))
        .with_project(session.project_id().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Arc;

    #[test]
    fn conversation_creation_is_idempotent() {
        let connection = Connection::open_in_memory().expect("in-memory");
        crate::persistence::schema::initialize_database(&connection).expect("schema");
        let writer = SqliteWriter::from_connection(connection);
        create_conversation(&writer, "conv-mcp").expect("first");
        create_conversation(&writer, "conv-mcp").expect("second");
        let count: i64 = writer
            .read_serialized(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM conversations WHERE id = 'conv-mcp'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())
            })
            .expect("count");
        assert_eq!(count, 1);
    }

    #[test]
    fn missing_project_is_not_authorized() {
        let connection = Connection::open_in_memory().expect("in-memory");
        crate::persistence::schema::initialize_database(&connection).expect("schema");
        let writer = Arc::new(SqliteWriter::from_connection(connection));
        assert!(!project_exists(&writer, "missing"));
    }
}
