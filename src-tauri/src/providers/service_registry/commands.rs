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
    state.sqlite_readers.read(|db| {
        let loaded = store::load_registry(db)?;
        let mut value = view(&loaded)?;
        value["latestUsage"] = json!(super::operations::latest(db, &loaded.snapshot)?);
        value["probes"] = json!(super::probe::latest(db, &loaded.snapshot)?);
        Ok(value)
    })
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
    let mut value = view(&loaded)?;
    value["latestUsage"] = json!(state
        .sqlite_readers
        .read(|db| super::operations::latest(db, &loaded.snapshot))?);
    value["probes"] = json!(state
        .sqlite_readers
        .read(|db| super::probe::latest(db, &loaded.snapshot))?);
    Ok(value)
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

#[tauri::command]
pub async fn probe_service_resource(
    state: tauri::State<'_, AppState>,
    resource_id: String,
    kind: String,
) -> Result<Value, String> {
    if !matches!(kind.as_str(), "models" | "generation") {
        return Err("Unknown diagnostic".into());
    }
    let loaded = state.sqlite_readers.read(store::load_registry)?;
    let resource = loaded
        .snapshot
        .resource(&resource_id)
        .ok_or("Resource is missing")?;
    let fingerprint = super::probe::fingerprint(&loaded.snapshot, resource)?;
    let result = super::probe::run(&loaded.snapshot, resource, &kind).await;
    let attributes = json!({"resourceId":resource_id,"fingerprint":fingerprint,"kind":kind});
    state.sqlite_writer.write(|db| {
        db.execute("INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json) VALUES(?1,?2,'provider','service-resource-probe','terminal',?3,?4)",rusqlite::params![format!("audit_{}",uuid::Uuid::new_v4().simple()),crate::now_iso(),if result.is_ok(){"success"}else{"failure"},attributes.to_string()]).map_err(crate::database_error)?;
        Ok(())
    })?;
    result
}
