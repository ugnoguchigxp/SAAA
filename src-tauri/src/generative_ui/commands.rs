use super::{contracts::*, data, parser, store};
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

#[tauri::command]
pub(crate) fn get_ui_enabled(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    state.sqlite_readers.read(store::enabled)
}
#[tauri::command]
pub(crate) fn set_ui_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    state.sqlite_writer.write(|c| {
        c.execute("UPDATE ui_settings SET enabled=?1 WHERE id=1", [enabled])
            .map_err(database_error)?;
        Ok(())
    })
}
#[tauri::command]
pub(crate) fn get_ui_instance(
    state: tauri::State<'_, AppState>,
    instance_id: String,
) -> Result<UiInstance, String> {
    state.sqlite_readers.read(|c| store::load(c, &instance_id))
}
#[tauri::command]
pub(crate) fn query_ui_source(
    state: tauri::State<'_, AppState>,
    instance_id: String,
    source: String,
) -> Result<UiData, String> {
    state.sqlite_readers.read(|c| {
        store::require_enabled(c)?;
        let instance = store::load(c, &instance_id)?;
        if instance.mode != "live" || !parser::sources(&instance.node).contains(&source) {
            return Err("Source is not available to this UI".into());
        }
        data::query(c, &store::conversation(c, &instance_id)?, &source)
    })
}
#[tauri::command]
pub(crate) fn save_ui_instance_state(
    state: tauri::State<'_, AppState>,
    instance_id: String,
    expected_version: u32,
    value: Value,
) -> Result<u32, String> {
    validate_state(&value)?;
    state.sqlite_writer.write(|c| {
        let changed = c.execute("UPDATE ui_instances SET state_json=?1,state_version=state_version+1 WHERE id=?2 AND state_version=?3", params![value.to_string(),instance_id,expected_version]).map_err(database_error)?;
        if changed != 1 { return Err("UI state conflict".into()); }
        Ok(expected_version+1)
    })
}
pub(crate) fn validate_state(value: &Value) -> Result<(), String> {
    let obj = value.as_object().ok_or("Invalid UI state")?;
    if obj.len() > 100
        || value.to_string().len() > 16_384
        || obj.iter().any(|(k, v)| {
            k.len() > 160
                || !(v.is_number()
                    || v.is_boolean()
                    || v.as_str().is_some_and(|s| s.chars().count() <= 1000))
        })
    {
        return Err("UI state exceeds limits".into());
    }
    Ok(())
}
#[tauri::command]
pub(crate) fn publish_ui_view(
    state: tauri::State<'_, AppState>,
    input: SaveInput,
) -> Result<Value, String> {
    state.sqlite_writer.write(|c| store::save(c, input))
}
#[tauri::command]
pub(crate) fn search_ui_views(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<SavedView>, String> {
    state.sqlite_readers.read(|c| store::search(c, &query))
}
#[tauri::command]
pub(crate) fn open_ui_view(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    view_id: String,
) -> Result<Value, String> {
    state.sqlite_writer.write(|c| {
        let tx = c.transaction().map_err(database_error)?;
        let result = store::open(&tx, &conversation_id, &view_id)?;
        tx.commit().map_err(database_error)?;
        Ok(result)
    })
}
#[tauri::command]
pub(crate) fn snapshot_ui_view(
    state: tauri::State<'_, AppState>,
    instance_id: String,
) -> Result<Value, String> {
    state.sqlite_writer.write(|c| {
        store::require_enabled(c)?;
        let tx = c.transaction().map_err(database_error)?;
        let instance = store::load(&tx, &instance_id)?;
        let result = store::create_instance(
            &tx,
            &store::conversation(&tx, &instance_id)?,
            &instance.view_id,
            instance.revision,
            "snapshot",
            &instance.node,
        )?;
        tx.commit().map_err(database_error)?;
        Ok(result)
    })
}

#[tauri::command]
pub(crate) fn cancel_ui_run(
    state: tauri::State<'_, AppState>,
    instance_id: String,
    target_id: String,
    request_id: String,
) -> Result<Value, String> {
    cancel_for_state(&state, instance_id, target_id, request_id)
}
pub(crate) fn cancel_for_state(
    state: &AppState,
    instance_id: String,
    target_id: String,
    request_id: String,
) -> Result<Value, String> {
    crate::validate_identifier(&request_id, "request id")?;
    state.sqlite_writer.write(|c| {
        store::require_enabled(c)?;
        let instance=store::load(c,&instance_id)?;
        fn allows(node:&UiNode)->bool { (node.kind=="Actions" && node.args.first().is_some_and(|a|a=="cancel_run")) || node.children.iter().any(allows) }
        if instance.mode!="live" || !allows(&instance.node) || !parser::sources(&instance.node).contains(&"runtime.runs".to_string()) { return Err("Action is not available to this UI".into()); }
        let old: Option<(String,String,String)> = c.query_row("SELECT instance_id,target_id,status FROM ui_action_results WHERE request_id=?1", [&request_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(database_error)?;
        if let Some((old_instance,old_target,status))=old {
            if old_instance!=instance_id || old_target!=target_id { return Err("Action request conflict".into()); }
            return Ok(json!({"status":status}));
        }
        let conversation=store::conversation(c,&instance_id)?;
        let running: bool=c.query_row("SELECT EXISTS(SELECT 1 FROM runtime_runs WHERE id=?1 AND conversation_id=?2 AND status='running')",params![target_id,conversation],|r|r.get(0)).map_err(database_error)?;
        if !running { return Err("Run is no longer active".into()); }
        let cancellation=state.active_runs.lock().map_err(|_|"Run state unavailable")?.get(&target_id).cloned().ok_or("Run is no longer active")?;
        let tx=c.transaction().map_err(database_error)?;
        tx.execute("INSERT INTO ui_action_results VALUES(?1,?2,?3,'accepted')",params![request_id,instance_id,target_id]).map_err(database_error)?;
        tx.execute("INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json) VALUES(?1,?2,'conversation','ui-cancel-run','terminal','success',?3)",params![new_id("audit"),now_iso(),json!({"requestId":request_id,"targetId":target_id}).to_string()]).map_err(database_error)?;
        tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'assistant',?3,?4)",params![new_id("message"),conversation,format!("実行の停止を要求しました / Cancellation requested: {target_id}"),now_iso()]).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        cancellation.cancel();
        Ok(json!({"status":"accepted"}))
    })
}

#[tauri::command]
pub(crate) fn archive_ui_view(
    state: tauri::State<'_, AppState>,
    view_id: String,
) -> Result<(), String> {
    state.sqlite_writer.write(|c| store::archive(c, &view_id))
}
