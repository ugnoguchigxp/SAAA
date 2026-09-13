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
        let voice_profile = state
            .voice_profile
            .read_with_snapshot(&state.sqlite_readers, |_connection, snapshot| Ok(snapshot))?;
        if !voice_profile.runtime_available {
            return Err(format!(
                "Packaged speaker verification is unavailable: {}",
                voice_profile.runtime_message
            ));
        }
    }
    if env::var_os("SAAA_SMOKE_EXERCISE_SITUATION").is_some() {
        state.situation.set_monitoring(&state.sqlite_writer, true)?;
        let sample = state.situation.sample_platform()?;
        state.situation.tick_sampled(&state.sqlite_writer, sample)?;
        state
            .situation
            .set_monitoring(&state.sqlite_writer, false)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;

    fn state() -> crate::AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    #[test]
    fn frontend_ready_is_a_no_op_without_a_smoke_marker() {
        frontend_ready(&state()).expect("idle frontend ready succeeds");
    }

    #[test]
    fn readiness_data_directory_must_be_a_private_directory_distinct_from_app_data() {
        assert!(
            validate_readiness_data_directory(Path::new("relative"), Path::new("/tmp")).is_err()
        );
        let missing = std::env::temp_dir().join(format!(
            "saaa-missing-readiness-{}",
            uuid::Uuid::new_v4().simple()
        ));
        assert!(validate_readiness_data_directory(&missing, Path::new("/tmp")).is_err());

        let directory = tempfile::tempdir().expect("temporary directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("private mode");
        }
        let canonical = validate_readiness_data_directory(directory.path(), Path::new("/tmp"))
            .expect("private directory is accepted");
        assert_eq!(
            canonical,
            fs::canonicalize(directory.path()).expect("canonical path")
        );
        assert!(validate_readiness_data_directory(directory.path(), directory.path()).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
                .expect("relaxed mode");
            assert!(
                validate_readiness_data_directory(directory.path(), Path::new("/tmp")).is_err()
            );
        }
    }
}
