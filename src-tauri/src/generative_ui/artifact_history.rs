use crate::{database_error, AppState};
use serde_json::{json, Value};

#[tauri::command]
pub(crate) fn list_artifact_instances(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    state.sqlite_readers.read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT i.id,i.conversation_id,i.view_id,i.revision,r.summary,v.name,m.created_at
                 FROM ui_instances i
                 JOIN conversation_message_parts p ON p.instance_id=i.id
                 JOIN conversation_messages m ON m.id=p.message_id
                 JOIN ui_view_revisions r ON r.view_id=i.view_id AND r.revision=i.revision
                 LEFT JOIN ui_views v ON v.id=i.view_id
                 ORDER BY CAST(m.created_at AS INTEGER) DESC,m.id DESC LIMIT 200",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(json!({
                    "instanceId": row.get::<_, String>(0)?,
                    "conversationId": row.get::<_, String>(1)?,
                    "viewId": row.get::<_, String>(2)?,
                    "revision": row.get::<_, u32>(3)?,
                    "summary": row.get::<_, String>(4)?,
                    "name": row.get::<_, Option<String>>(5)?,
                    "createdAt": row.get::<_, String>(6)?,
                }))
            })
            .map_err(database_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
    })
}

