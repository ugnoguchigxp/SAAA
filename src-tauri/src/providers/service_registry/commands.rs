use super::{RegistrySnapshot, SERVICE_CREDENTIAL_SERVICE};
use crate::persistence::service_registry_store as store;
use crate::{credentials, AppState};
use serde_json::{json, Value};

fn view(loaded: &store::LoadedRegistry) -> Result<Value, String> {
    Ok(json!({
        "snapshot": serde_json::to_value(&loaded.snapshot).map_err(|error| error.to_string())?,
        "revision": loaded.revision,
        "persisted": loaded.persisted,
    }))
}

#[tauri::command]
pub fn get_service_registry(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    view(&state.sqlite_readers.read(store::load_registry)?)
}

#[tauri::command]
pub fn save_service_registry(
    state: tauri::State<'_, AppState>,
    snapshot: Value,
    expected_revision: i64,
) -> Result<Value, String> {
    let snapshot: RegistrySnapshot = serde_json::from_value(snapshot)
        .map_err(|error| format!("Invalid service registry: {error}"))?;
    let loaded = state
        .sqlite_writer
        .write(|connection| store::save_registry(connection, &snapshot, expected_revision))?;
    view(&loaded)
}

fn secret_target(state: &AppState, connection_id: &str) -> Result<super::CredentialRef, String> {
    let loaded = state.sqlite_readers.read(store::load_registry)?;
    if !loaded.persisted {
        return Err("Save the connection before storing its API key".to_string());
    }
    loaded
        .snapshot
        .connection(connection_id)
        .and_then(|connection| connection.credential_ref.clone())
        .ok_or_else(|| "The connection does not use a stored API key".to_string())
}

#[tauri::command]
pub fn set_service_connection_secret(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    api_key: String,
) -> Result<Value, String> {
    let api_key = zeroize::Zeroizing::new(api_key);
    credentials::validate_api_key(&api_key)?;
    let target = secret_target(&state, &connection_id)?;
    credentials::store_named_secret(&target.service, &target.account, api_key.as_bytes())?;
    Ok(json!({"connectionId": connection_id, "state": "configured"}))
}

#[tauri::command]
pub fn get_service_connection_secret_state(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> Result<Value, String> {
    let target = secret_target(&state, &connection_id)?;
    let configured = credentials::load_named_secret(&target.service, &target.account)?.is_some();
    Ok(json!({
        "connectionId": connection_id,
        "state": if configured { "configured" } else { "missing" },
    }))
}

/// Credential service for connections created through the registry.
#[allow(dead_code)]
pub(crate) const fn credential_service() -> &'static str {
    SERVICE_CREDENTIAL_SERVICE
}
