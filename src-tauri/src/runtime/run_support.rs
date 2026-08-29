use rusqlite::params;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use crate::{
    database_error, now_iso, validate_identifier, AppState, RunCancellation, StartTurnInput,
};

pub(crate) fn register_active_run(
    state: &AppState,
    run_id: &str,
    cancellation: Arc<RunCancellation>,
) -> Result<(), String> {
    if crate::memory::control_plane::memory_enabled() {
        if let Ok(connection) = state.connection.lock() {
            let _ = crate::memory::control_plane::cancel_running_jobs(&connection, &now_iso());
        }
    }
    let mut active = state
        .active_runs
        .lock()
        .map_err(|_| "Runtime run lock unavailable".to_string())?;
    match active.entry(run_id.to_string()) {
        Entry::Vacant(entry) => {
            entry.insert(cancellation);
        }
        Entry::Occupied(_) => return Err("A run with this id is already active".to_string()),
    }
    Ok(())
}

pub(crate) fn remove_active_run(state: &AppState, run_id: &str) {
    if let Ok(mut active) = state.active_runs.lock() {
        active.remove(run_id);
    }
}

pub(crate) fn begin_simple_runtime_run(
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
    route_kind: &str,
    provider_id: &str,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let conversation_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            params![conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !conversation_exists {
        return Err("Conversation does not exist".to_string());
    }
    connection
        .execute(
            "INSERT INTO runtime_runs(id, conversation_id, route_kind, provider_id, status, started_at)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
            params![run_id, conversation_id, route_kind, provider_id, now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn validate_start_turn(input: &StartTurnInput) -> Result<(), String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    let content = input.content.trim();
    if content.is_empty() || content.chars().count() > 16_000 {
        return Err("Message must contain between 1 and 16,000 characters".to_string());
    }
    if let Some(workspace) = &input.workspace_path {
        if workspace.len() > 4_096 {
            return Err("Workspace path is too long".to_string());
        }
    }
    Ok(())
}

pub(crate) fn update_runtime_provider(
    state: &AppState,
    run_id: &str,
    provider_id: &str,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE runtime_runs SET provider_id = ?1 WHERE id = ?2 AND status = 'running'",
            params![provider_id, run_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run is not active".to_string());
    }
    Ok(())
}
