use std::fs;

use crate::{backup::backup_connection_to, AppState, LocalArtifactResult};

#[tauri::command]
pub(crate) fn backup_database(
    state: tauri::State<'_, AppState>,
) -> Result<LocalArtifactResult, String> {
    let created_at = crate::now_iso();
    let directory = state.data_directory.join("backups");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the backup directory: {error}"))?;
    let path = directory.join(format!("saaa-{created_at}.sqlite3"));
    state
        .sqlite_writer
        .read_serialized(|source| backup_connection_to(source, &path))?;
    Ok(LocalArtifactResult {
        path: path.to_string_lossy().into_owned(),
        created_at,
    })
}
