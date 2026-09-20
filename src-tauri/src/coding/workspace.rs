use crate::{database_error, new_id};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn replace(
    connection: &Connection,
    conversation: &str,
    path: &str,
) -> Result<String, String> {
    let previous: Option<String> = connection
        .query_row(
            "SELECT id FROM coding_workspaces WHERE conversation_id=?1",
            [conversation],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    let id = new_id("workspace");
    connection
        .execute(
            "INSERT INTO coding_workspaces(id,conversation_id,path) VALUES(?1,?2,?3)
             ON CONFLICT(conversation_id) DO UPDATE SET id=excluded.id,path=excluded.path",
            params![id, conversation, path],
        )
        .map_err(database_error)?;
    if let Some(previous) = previous.filter(|previous| previous != &id) {
        crate::runtime::context::scope::revoke(connection, &format!("resource:{previous}"))?;
        crate::runtime::context::scope::revoke(connection, &format!("project:{previous}"))?;
    }
    // A workspace is the durable root for the user's active coding project. The resource remains
    // distinct so task/resource authorization does not get conflated with project focus.
    let project_scope = crate::runtime::context::scope::register(connection, "project", &id)?;
    crate::runtime::context::scope::register(connection, "resource", &id)?;
    crate::runtime::context::scope::link(connection, &project_scope, &format!("resource:{id}"))?;
    Ok(id)
}
