use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use tauri::Manager;

use crate::validate_identifier;
use crate::AppState;

pub(crate) fn frontend_ready(state: &AppState) -> Result<(), String> {
    let Some(marker_id) = env::var("SAAA_SMOKE_MARKER_ID")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    validate_identifier(&marker_id, "smoke marker id")?;
    if env::var_os("SAAA_SMOKE_REQUIRE_SPEAKER").is_some() {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        let voice_profile = state.voice_profile.snapshot(&connection)?;
        if !voice_profile.runtime_available {
            return Err(format!(
                "Packaged speaker verification is unavailable: {}",
                voice_profile.runtime_message
            ));
        }
    }
    if env::var_os("SAAA_SMOKE_EXERCISE_SITUATION").is_some() {
        state.situation.set_monitoring(&state.connection, true)?;
        let sample = state.situation.sample_platform()?;
        state.situation.tick_sampled(&state.connection, sample)?;
        state.situation.set_monitoring(&state.connection, false)?;
    }
    fs::write(
        env::temp_dir().join(format!("saaa-frontend-{marker_id}.ready")),
        "ready",
    )
    .map_err(|error| format!("Could not write the frontend smoke marker: {error}"))
}

pub(crate) fn application_database_path(
    app: &tauri::App,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if env::var_os("SAAA_SMOKE_MARKER_ID").is_some() {
        if let Some(directory) = env::var_os("SAAA_SMOKE_DATA_DIR").map(PathBuf::from) {
            if !directory.is_absolute() {
                return Err("SAAA_SMOKE_DATA_DIR must be absolute".into());
            }
            fs::create_dir_all(&directory)?;
            return Ok(directory.join("saaa.sqlite3"));
        }
    }
    let directory = app.path().app_data_dir()?;
    if let Some(readiness_directory) = env::var_os("SAAA_MVP2X_APP_DATA_DIR").map(PathBuf::from) {
        let readiness_directory =
            validate_readiness_data_directory(&readiness_directory, &directory)
                .map_err(std::io::Error::other)?;
        return Ok(readiness_directory.join("saaa.sqlite3"));
    }
    fs::create_dir_all(&directory)?;
    Ok(directory.join("saaa.sqlite3"))
}

pub(crate) fn validate_readiness_data_directory(
    directory: &Path,
    normal_app_data: &Path,
) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("SAAA_MVP2X_APP_DATA_DIR must be absolute".to_string());
    }
    let metadata = fs::symlink_metadata(directory)
        .map_err(|_| "SAAA_MVP2X_APP_DATA_DIR must be an existing directory".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("SAAA_MVP2X_APP_DATA_DIR must be a real directory".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err("SAAA_MVP2X_APP_DATA_DIR must have mode 0700".to_string());
        }
    }
    let canonical = fs::canonicalize(directory)
        .map_err(|_| "SAAA_MVP2X_APP_DATA_DIR could not be resolved".to_string())?;
    let normal =
        fs::canonicalize(normal_app_data).unwrap_or_else(|_| normal_app_data.to_path_buf());
    if canonical == normal {
        return Err("SAAA_MVP2X_APP_DATA_DIR must not use normal application data".to_string());
    }
    Ok(canonical)
}
