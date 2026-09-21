use super::{contracts::*, parser};
use crate::{database_error, new_id, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS ui_settings(id INTEGER PRIMARY KEY CHECK(id=1),enabled INTEGER NOT NULL CHECK(enabled IN (0,1)));
      INSERT OR IGNORE INTO ui_settings VALUES(1,0);
      CREATE TABLE IF NOT EXISTS ui_views(id TEXT PRIMARY KEY,name TEXT,description TEXT NOT NULL DEFAULT '',tags_json TEXT NOT NULL DEFAULT '[]',current_revision INTEGER NOT NULL,published_revision INTEGER,status TEXT NOT NULL CHECK(status IN ('ephemeral','draft','published','archived')));
      CREATE TABLE IF NOT EXISTS ui_view_revisions(view_id TEXT NOT NULL REFERENCES ui_views(id) ON DELETE CASCADE,revision INTEGER NOT NULL,definition TEXT NOT NULL,summary TEXT NOT NULL,library_version INTEGER NOT NULL DEFAULT 1,created_at TEXT NOT NULL,PRIMARY KEY(view_id,revision));
      CREATE TABLE IF NOT EXISTS ui_instances(id TEXT PRIMARY KEY,view_id TEXT NOT NULL,revision INTEGER NOT NULL,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,mode TEXT NOT NULL CHECK(mode IN ('live','snapshot')),state_json TEXT NOT NULL DEFAULT '{}',state_version INTEGER NOT NULL DEFAULT 0,snapshots_json TEXT NOT NULL DEFAULT '{}',FOREIGN KEY(view_id,revision) REFERENCES ui_view_revisions(view_id,revision));
      CREATE TABLE IF NOT EXISTS conversation_message_parts(message_id TEXT PRIMARY KEY REFERENCES conversation_messages(id) ON DELETE CASCADE,instance_id TEXT NOT NULL REFERENCES ui_instances(id) ON DELETE CASCADE);
      CREATE TABLE IF NOT EXISTS ui_tool_results(run_id TEXT NOT NULL REFERENCES runtime_runs(id) ON DELETE CASCADE,call_id TEXT NOT NULL,result_json TEXT NOT NULL,PRIMARY KEY(run_id,call_id));
      CREATE TABLE IF NOT EXISTS ui_action_results(request_id TEXT PRIMARY KEY,instance_id TEXT NOT NULL REFERENCES ui_instances(id) ON DELETE CASCADE,target_id TEXT NOT NULL,status TEXT NOT NULL);
      CREATE INDEX IF NOT EXISTS idx_ui_instances_conversation ON ui_instances(conversation_id);
      CREATE TRIGGER IF NOT EXISTS ui_collect_instance AFTER DELETE ON conversation_message_parts BEGIN
        DELETE FROM ui_instances WHERE id=OLD.instance_id;
        DELETE FROM ui_views WHERE published_revision IS NULL AND NOT EXISTS(SELECT 1 FROM ui_instances WHERE view_id=ui_views.id);
      END;")
}

pub(crate) fn enabled(connection: &Connection) -> Result<bool, String> {
    connection
        .query_row("SELECT enabled FROM ui_settings WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(database_error)
}
pub(crate) fn require_enabled(connection: &Connection) -> Result<(), String> {
    if enabled(connection)? {
        Ok(())
    } else {
        Err("Generative UI is disabled".into())
    }
}
pub(crate) fn conversation(connection: &Connection, instance: &str) -> Result<String, String> {
    connection
        .query_row(
            "SELECT conversation_id FROM ui_instances WHERE id=?1",
            [instance],
            |r| r.get(0),
        )
        .map_err(database_error)
}
pub(crate) fn load(connection: &Connection, id: &str) -> Result<UiInstance, String> {
    load_revision(connection, id, None)
}

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

pub(crate) fn create(
    connection: &Connection,
    conversation: &str,
    input: PresentInput,
) -> Result<Value, String> {
    require_enabled(connection)?;
    if input.summary.trim().is_empty()
        || input.summary.chars().count() > 1000
        || !["live", "snapshot"].contains(&input.mode.as_str())
    {
        return Err("Invalid UI summary or mode".into());
    }
    let node = parser::parse(&input.definition)?;
    let definition = serde_json::to_string(&node).map_err(|_| "UI encoding failed")?;
    if definition.len() > 65_536 {
        return Err("Stored UI definition exceeds size limit".into());
    }
    let (view, revision) = if let Some(base) = &input.base_instance_id {
        let instance = load(connection, base)?;
        if self::conversation(connection, base)? != conversation {
            return Err("Edit target belongs to another conversation".into());
        }
        let changed = connection.execute("UPDATE ui_views SET current_revision=current_revision+1,status='draft' WHERE id=?1 AND current_revision=?2", params![instance.view_id,instance.revision]).map_err(database_error)?;
        if changed != 1 {
            return Err("Revision conflict: reload the current view".into());
        }
        (instance.view_id, instance.revision + 1)
    } else {
        let view = new_id("view");
        connection
            .execute(
                "INSERT INTO ui_views(id,current_revision,status) VALUES(?1,1,'ephemeral')",
                [&view],
            )
            .map_err(database_error)?;
        (view, 1)
    };
    connection.execute("INSERT INTO ui_view_revisions(view_id,revision,definition,summary,created_at) VALUES(?1,?2,?3,?4,?5)", params![view,revision,definition,input.summary,now_iso()]).map_err(database_error)?;
    create_instance(
        connection,
        conversation,
        &view,
        revision,
        &input.mode,
        &node,
    )
}

pub(crate) fn create_instance(
    connection: &Connection,
    conversation: &str,
    view: &str,
    revision: u32,
    mode: &str,
    node: &UiNode,
) -> Result<Value, String> {
    let task_mode: String = connection
        .query_row(
            "SELECT task_mode FROM conversations WHERE id=?1",
            [conversation],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    crate::persistence::conversations::validate_conversation_write_target(
        conversation,
        &task_mode,
    )?;
    let id = new_id("ui");
    let summary: String = connection
        .query_row(
            "SELECT summary FROM ui_view_revisions WHERE view_id=?1 AND revision=?2",
            params![view, revision],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    let mut snapshots = json!({});
    if mode == "snapshot" {
        for source in parser::sources(node) {
            snapshots[&source] =
                serde_json::to_value(super::data::query(connection, conversation, &source)?)
                    .map_err(|_| "Snapshot encoding failed")?;
        }
        if snapshots.to_string().len() > 1_048_576 {
            return Err("Snapshot exceeds size limit".into());
        }
    }
    connection.execute("INSERT INTO ui_instances(id,view_id,revision,conversation_id,mode,snapshots_json) VALUES(?1,?2,?3,?4,?5,?6)", params![id,view,revision,conversation,mode,snapshots.to_string()]).map_err(database_error)?;
    let message = new_id("message");
    let now = now_iso();
    connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'assistant',?3,?4)", params![message,conversation,summary,now]).map_err(database_error)?;
    connection
        .execute(
            "INSERT INTO conversation_message_parts(message_id,instance_id) VALUES(?1,?2)",
            params![message, id],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![now, conversation],
        )
        .map_err(database_error)?;
    Ok(
        json!({"instanceId":id,"viewId":view,"revision":revision,"summary":summary,"messageId":message}),
    )
}

pub(crate) fn save(connection: &Connection, input: SaveInput) -> Result<Value, String> {
    require_enabled(connection)?;
    if input.name.trim().is_empty()
        || input.name.chars().count() > 120
        || input.description.chars().count() > 1000
        || input.tags.len() > 12
        || input.tags.iter().any(|t| t.chars().count() > 64)
    {
        return Err("Invalid saved view metadata".into());
    }
    let instance = load(connection, &input.instance_id)?;
    let changed = connection.execute("UPDATE ui_views SET name=?1,description=?2,tags_json=?3,published_revision=?4,status='published' WHERE id=?5 AND current_revision=?4", params![input.name,input.description,serde_json::to_string(&input.tags).unwrap(),instance.revision,instance.view_id]).map_err(database_error)?;
    if changed != 1 {
        return Err("Revision conflict: only the current revision can be published".into());
    }
    Ok(json!({"saved":true,"viewId":instance.view_id,"revision":instance.revision}))
}

pub(crate) fn search(connection: &Connection, query: &str) -> Result<Vec<SavedView>, String> {
    if query.chars().count() > 256 {
        return Err("Search query too long".into());
    }
    // Literal substring matching supports Japanese without assuming an FTS tokenizer.
    let mut stmt = connection.prepare("SELECT id,name,description,tags_json,published_revision FROM ui_views WHERE published_revision IS NOT NULL AND status!='archived' AND (instr(lower(name),lower(?1))>0 OR instr(lower(description),lower(?1))>0 OR instr(lower(tags_json),lower(?1))>0) ORDER BY name,id LIMIT 50").map_err(database_error)?;
    let rows = stmt
        .query_map([query], |r| {
            Ok(SavedView {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                tags: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                revision: r.get(4)?,
            })
        })
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
}

pub(crate) fn open(
    connection: &Connection,
    conversation: &str,
    view: &str,
) -> Result<Value, String> {
    require_enabled(connection)?;
    let (revision, definition): (u32,String) = connection.query_row("SELECT v.published_revision,r.definition FROM ui_views v JOIN ui_view_revisions r ON r.view_id=v.id AND r.revision=v.published_revision WHERE v.id=?1 AND v.status!='archived'", [view], |r| Ok((r.get(0)?,r.get(1)?))).map_err(database_error)?;
    create_instance(
        connection,
        conversation,
        view,
        revision,
        "live",
        &parser::parse(&definition)?,
    )
}

pub(crate) fn hydrate(
    connection: &Connection,
    messages: &mut [crate::ipc_contract::ConversationMessage],
) -> Result<(), String> {
    if messages.is_empty() {
        return Ok(());
    }
    let ids = messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>();
    let placeholders = vec!["?"; ids.len()].join(",");
    let mut stmt = connection.prepare(&format!("SELECT p.message_id,i.id,i.view_id,i.revision FROM conversation_message_parts p JOIN ui_instances i ON i.id=p.instance_id WHERE p.message_id IN ({placeholders})")).map_err(database_error)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(ids), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u32>(3)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        let (message_id, id, view, revision) = row.map_err(database_error)?;
        if let Some(message) = messages.iter_mut().find(|m| m.id == message_id) {
            message.parts = Some(vec![ContentPart::Ui {
                instance_id: id,
                view_id: view,
                revision,
                summary: message.content.clone(),
            }]);
        }
    }
    Ok(())
}

pub(crate) fn cached(
    connection: &Connection,
    run: &str,
    call: &str,
) -> Result<Option<Value>, String> {
    let result: Option<String> = connection
        .query_row(
            "SELECT result_json FROM ui_tool_results WHERE run_id=?1 AND call_id=?2",
            params![run, call],
            |r| r.get(0),
        )
        .optional()
        .map_err(database_error)?;
    result
        .map(|s| serde_json::from_str(&s).map_err(|_| "Invalid cached UI result".into()))
        .transpose()
}

pub(crate) fn archive(connection: &Connection, view: &str) -> Result<(), String> {
    require_enabled(connection)?;
    let changed = connection
        .execute(
            "UPDATE ui_views SET status='archived' WHERE id=?1 AND published_revision IS NOT NULL",
            [view],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Saved view unavailable".into());
    }
    Ok(())
}
