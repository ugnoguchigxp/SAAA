//! IPC wrappers (docs/plans/worker-agents.md §4.3). User/host operations only.
use super::{queries, repository};
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::now_ms;
use crate::{validate_identifier, AppState};

#[tauri::command]
pub(crate) fn list_worker_agents(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<WorkerAgentSummary>, String> {
    state.sqlite_readers.read(queries::list_agents)
}

#[tauri::command]
pub(crate) fn get_worker_agent(
    state: tauri::State<'_, AppState>,
    profile_id: String,
) -> Result<WorkerAgentDetail, String> {
    state
        .sqlite_readers
        .read(|connection| queries::get_agent(connection, &profile_id))
}

#[tauri::command]
pub(crate) fn save_worker_agent_draft(
    state: tauri::State<'_, AppState>,
    draft: ProfileDraft,
) -> Result<RevisionSummary, String> {
    state
        .sqlite_writer
        .transact(|connection| repository::save_draft(connection, &draft, "user_ipc", now_ms()))
}

#[tauri::command]
pub(crate) fn approve_worker_agent_revision(
    state: tauri::State<'_, AppState>,
    profile_id: String,
    revision_id: String,
    definition_hash: String,
) -> Result<WorkerAgentSummary, String> {
    validate_identifier(&profile_id, "profile id")?;
    validate_identifier(&revision_id, "revision id")?;
    state.sqlite_writer.transact(|connection| {
        repository::approve_revision(
            connection,
            &profile_id,
            &revision_id,
            &definition_hash,
            now_ms(),
        )
    })
}

#[tauri::command]
pub(crate) fn set_worker_agent_enabled(
    state: tauri::State<'_, AppState>,
    profile_id: String,
    enabled: bool,
) -> Result<WorkerAgentSummary, String> {
    validate_identifier(&profile_id, "profile id")?;
    state
        .sqlite_writer
        .transact(|connection| repository::set_enabled(connection, &profile_id, enabled, now_ms()))
}

#[tauri::command]
pub(crate) fn save_worker_skill(
    state: tauri::State<'_, AppState>,
    draft: SkillDraft,
) -> Result<SkillSaved, String> {
    state
        .sqlite_writer
        .transact(|connection| repository::save_skill(connection, &draft, now_ms()))
}

#[tauri::command]
pub(crate) fn set_worker_web_search_mode(
    state: tauri::State<'_, AppState>,
    mode: WebSearchMode,
) -> Result<WebSearchMode, String> {
    state
        .sqlite_writer
        .transact(|connection| repository::set_web_search_mode(connection, mode))
}

#[tauri::command]
pub(crate) fn list_worker_tasks(
    state: tauri::State<'_, AppState>,
    limit: u32,
) -> Result<Vec<WorkerTaskSummary>, String> {
    state
        .sqlite_readers
        .read(|connection| queries::list_tasks(connection, limit))
}

#[tauri::command]
pub(crate) fn list_worker_url_blocklist(
    state: tauri::State<'_, AppState>,
    limit: u32,
    after: Option<String>,
) -> Result<Vec<BlocklistEntry>, String> {
    state
        .sqlite_readers
        .read(|connection| queries::list_blocklist(connection, limit, after.as_deref()))
}

#[tauri::command]
pub(crate) fn remove_worker_url_blocklist(
    state: tauri::State<'_, AppState>,
    url_hash: String,
) -> Result<bool, String> {
    state
        .sqlite_writer
        .transact(|connection| queries::remove_blocklist(connection, &url_hash))
}
