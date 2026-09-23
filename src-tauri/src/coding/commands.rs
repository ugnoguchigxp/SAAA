use super::{contracts::CodingSettings, repository as repo, service};
use crate::{database_error, AppState};
use serde_json::{json, Value};
#[tauri::command]
pub fn get_coding_settings(state: tauri::State<'_, AppState>) -> Result<CodingSettings, String> {
    state.sqlite_readers.read(repo::settings)
}
#[tauri::command]
pub fn save_coding_settings(
    state: tauri::State<'_, AppState>,
    settings: CodingSettings,
) -> Result<(), String> {
    if settings.version != "0.86.1"
        || !super::contracts::valid_implementation(&settings)
        || (settings.implementation_method == "pi" && !super::contracts::valid_profile(&settings))
        || settings.executable.len() > 4096
        || settings.provider.len() > 160
        || settings.model.len() > 160
    {
        return Err("coding_configuration_invalid".into());
    }
    state.sqlite_writer.write(|c| {
        c.execute(
            "UPDATE coding_settings SET value_json=?1 WHERE id=1",
            [serde_json::to_string(&settings).map_err(|_| "coding_configuration_invalid")?],
        )
        .map_err(database_error)?;
        Ok(())
    })
}
#[tauri::command]
pub async fn probe_coding(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let settings = state.sqlite_readers.read(repo::settings)?;
    if !super::contracts::valid_implementation(&settings) {
        return Err("coding_configuration_invalid".into());
    }
    if settings.implementation_method == "codex-sdk" {
        let mut child = crate::runtime::codex_cli::spawn_codex_app_server()?;
        child.kill().map_err(|_| "codex_probe_failed")?;
        child.wait().map_err(|_| "codex_probe_failed")?;
        return Ok(
            json!({"available":true,"method":"codex-sdk","model":settings.codex_model,"authentication":"existing Codex login; live request not tested"}),
        );
    }
    let directory = state.data_directory.clone();
    tauri::async_runtime::spawn_blocking(move||{
        crate::runtime::pi::process::check_settings(&settings)?;
        let temporary=tempfile::tempdir_in(&directory).map_err(|_|"probe_storage_unavailable")?;
        let session=temporary.path().join("probe.jsonl");let mut child=crate::runtime::pi::process::Process::open(&settings,temporary.path(),&session)?;
        let result=crate::runtime::pi::process::ready(&mut child,&settings,&session);let closed=child.close();result?;closed?;
        Ok(json!({"available":true,"version":settings.version,"provider":settings.provider,"model":settings.model,"authentication":"pi model registry available; live request not tested"}))
    }).await.map_err(|_|"probe_failed")?
}
#[tauri::command]
pub fn register_coding_workspace(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    path: String,
) -> Result<Value, String> {
    service::register(&state, &conversation_id, &path)
}
#[tauri::command]
pub fn coding_snapshot(state: tauri::State<'_, AppState>, conversation_id: String) -> Value {
    super::tools::context(&state, &conversation_id)
}
#[tauri::command]
pub fn cancel_coding_job(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    job_id: String,
    expected_revision: u64,
) -> Result<Value, String> {
    state.sqlite_writer.write(|c| {
        let tx = c.transaction().map_err(database_error)?;
        let value = service::cancel(
            &tx,
            &conversation_id,
            &job_id,
            expected_revision,
            "User requested stop",
        )?;
        tx.commit().map_err(database_error)?;
        Ok(value)
    })
}
pub fn shutdown(state: &AppState) {
    let _=state.sqlite_writer.write(|c|{c.execute("UPDATE coding_runs SET state='stopping',stop_reason='app_shutdown' WHERE state IN ('starting','running')",[]).map_err(database_error)?;c.execute("UPDATE coding_jobs SET state='cancel_requested' WHERE current_run_id IN (SELECT id FROM coding_runs WHERE state='stopping')",[]).map_err(database_error)?;Ok(())});
}
