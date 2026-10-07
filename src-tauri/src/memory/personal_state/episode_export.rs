use crate::{database_error, AppState};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn status(c: &Connection) -> Result<Value, String> {
    let mut stmt=c.prepare("SELECT s.scope_key,s.opaque_id,COALESCE(g.enabled,0) FROM context_scopes s LEFT JOIN memory_episode_grants g ON g.scope_key=s.scope_key WHERE s.state='active' ORDER BY s.scope_key").map_err(database_error)?;
    let items=stmt.query_map([],|r|Ok(json!({"scope":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"enabled":r.get::<_,bool>(2)?}))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
    let consolidation: bool = c
        .query_row(
            "SELECT enabled FROM personal_consolidation_settings WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    Ok(json!({"contractVersion":1,"scopes":items,"consolidationEnabled":consolidation}))
}
#[tauri::command]
pub fn episode_sync_status(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    state.sqlite_writer.read_serialized(status)
}
#[tauri::command]
pub fn set_episode_sync_scope(
    state: tauri::State<'_, AppState>,
    scope: String,
    enabled: bool,
) -> Result<Value, String> {
    state.sqlite_writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM context_scopes WHERE scope_key=?1 AND state='active')",[&scope],|r|r.get(0)).map_err(database_error)?;
        if !exists{return Err("episode-export-scope-unavailable".into());}
        tx.execute("INSERT INTO memory_episode_grants(scope_key,enabled) VALUES(?1,?2) ON CONFLICT(scope_key) DO UPDATE SET enabled=excluded.enabled,revision=memory_episode_grants.revision+1",params![scope,enabled]).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        status(c)
    })
}

fn validate_contract(
    c: &Connection,
    value: &Value,
    allowed: Option<&std::collections::BTreeSet<String>>,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let principal: String = c
        .query_row(
            "SELECT principal FROM personal_scope WHERE id='primary'",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if value["contractVersion"].as_u64() != Some(1)
        || value["principal"].as_str() != Some(&principal)
    {
        return Err("episode-source-contract".into());
    }
    let sources = value["sources"]
        .as_array()
        .filter(|s| !s.is_empty() && s.len() <= 8)
        .ok_or("episode-source-contract")?;
    for src in sources {
        let scope = src["scope"].as_str().ok_or("episode-source-scope")?;
        if allowed.is_some_and(|scopes| !scopes.contains(scope)) {
            return Err("episode-source-scope".into());
        }
        let source_id = src["sourceId"].as_str().ok_or("episode-source-id")?;
        let (version,sequence,policy,text,epoch,access):(u64,u64,u64,String,u64,u64)=c.query_row("SELECT version,sequence,policy_revision,content,scope_epoch,access_revision FROM memory_episode_source_v1 WHERE source_id=?1 AND scope_key=?2 ORDER BY version DESC LIMIT 1",params![source_id,scope],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(|_|"episode-source-unavailable")?;
        let start = src["start"].as_u64().ok_or("episode-source-range")? as usize;
        let end = src["end"].as_u64().ok_or("episode-source-range")? as usize;
        if src["accessRevision"].as_u64() != Some(access)
            || src["scopeEpoch"].as_u64() != Some(epoch)
            || src["byteLength"].as_u64() != Some(text.len() as u64)
            || start >= end
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            return Err("episode-source-range".into());
        }
        if src["version"].as_u64() != Some(version)
            || src["sequence"].as_u64() != Some(sequence)
            || src["policyRevision"].as_u64() != Some(policy)
            || src["digest"].as_str()
                != Some(format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
        {
            return Err("episode-source-changed".into());
        }
    }
    Ok(())
}
pub fn capture(c: &Connection, run: &str, content: &str, scopes: &[String]) -> Result<(), String> {
    let result: Value = serde_json::from_str(content).map_err(|_| "episode-result-json")?;
    let Some(items) = result["items"].as_array() else {
        return Ok(());
    };
    let allowed = scopes.iter().cloned().collect();
    for item in items {
        let Some(contract) = item.get("sourceContract").filter(|v| v.is_object()) else {
            continue;
        };
        validate_contract(c, contract, Some(&allowed))?;
        let id = item["id"].as_str().ok_or("episode-result-id")?;
        let key = item["sourceKey"].as_str().ok_or("episode-result-version")?;
        c.execute("INSERT INTO memory_episode_run_refs(run_id,episode_id,source_key,contract) VALUES(?1,?2,?3,?4) ON CONFLICT(run_id,episode_id) DO UPDATE SET source_key=excluded.source_key,contract=excluded.contract",params![run,id,key,contract.to_string()]).map_err(database_error)?;
    }
    Ok(())
}
pub fn validate_run(c: &Connection, run: &str) -> Result<(), String> {
    let mut s = c
        .prepare("SELECT contract FROM memory_episode_run_refs WHERE run_id=?1")
        .map_err(database_error)?;
    let rows = s
        .query_map([run], |r| r.get::<_, String>(0))
        .map_err(database_error)?;
    for row in rows {
        validate_contract(
            c,
            &serde_json::from_str(&row.map_err(database_error)?)
                .map_err(|_| "episode-source-contract")?,
            None,
        )?;
    }
    Ok(())
}

#[tauri::command]
pub fn set_memory_consolidation(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<Value, String> {
    state.sqlite_writer.transact(|c| {
        c.execute(
            "UPDATE personal_consolidation_settings SET enabled=?1 WHERE id=1",
            [enabled],
        )
        .map_err(database_error)?;
        status(c)
    })
}

#[path = "episode_export/source.rs"]
mod source;
pub(crate) use source::{capture_snapshot_inputs, fetch_source, source_definition, SOURCE_TOOL};

#[path = "episode_export/history.rs"]
mod history;
pub(crate) use history::{reuse as reuse_history, view as history_view};
