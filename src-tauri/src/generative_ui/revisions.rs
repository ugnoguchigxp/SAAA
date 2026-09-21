use super::{contracts::*, parser};
use crate::database_error;
use rusqlite::{params, Connection};
use serde_json::json;

pub(crate) fn load_revision(
    connection: &Connection,
    id: &str,
    revision: Option<u32>,
) -> Result<UiInstance, String> {
    let mut value = connection.query_row("SELECT i.id,i.view_id,i.revision,r.summary,r.definition,r.library_version,i.mode,i.state_json,i.snapshots_json,i.state_version,v.name,v.published_revision FROM ui_instances i JOIN ui_view_revisions r ON r.view_id=i.view_id AND r.revision=i.revision JOIN ui_views v ON v.id=i.view_id WHERE i.id=?1", [id], |r| {
        Ok(UiInstance { id:r.get(0)?,view_id:r.get(1)?,revision:r.get(2)?,summary:r.get(3)?,definition:r.get(4)?,library_version:r.get(5)?,mode:r.get(6)?,state:serde_json::from_str(&r.get::<_,String>(7)?).unwrap_or(json!({})),snapshots:serde_json::from_str(&r.get::<_,String>(8)?).unwrap_or(json!({})),state_version:r.get(9)?,name:r.get(10)?,published_revision:r.get(11)?,node:UiNode { id:String::new(),kind:String::new(),args:vec![],span:12,children:vec![] } })
    }).map_err(database_error)?;
    if let Some(revision) = revision {
        let (loaded_revision, summary, definition, library_version): (u32, String, String, u32) =
            connection
                .query_row(
                    "SELECT revision,summary,definition,library_version FROM ui_view_revisions WHERE view_id=?1 AND revision=?2",
                    params![value.view_id, revision],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(database_error)?;
        value.revision = loaded_revision;
        value.summary = summary;
        value.definition = definition;
        value.library_version = library_version;
    }
    if value.library_version != 1 {
        return Err("Unsupported UI library version".into());
    }
    value.node = parser::parse(&value.definition)?;
    Ok(value)
}

pub(crate) fn list_revisions(
    connection: &Connection,
    view_id: &str,
) -> Result<Vec<UiViewRevision>, String> {
    let mut statement = connection
        .prepare(
            "SELECT revision,summary,created_at FROM ui_view_revisions WHERE view_id=?1 ORDER BY revision DESC",
        )
        .map_err(database_error)?;
    let revisions = statement
        .query_map([view_id], |row| {
            Ok(UiViewRevision {
                revision: row.get(0)?,
                summary: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(revisions)
}
