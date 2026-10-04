//! Persistence for the purpose-based service registry.
//!
//! The registry lives in one `providers.registry/default` settings document so
//! connections, resources and bindings commit atomically through the single
//! writer. It is outside the complete-snapshot batch and the settings list
//! whitelist, so legacy settings saves are unaffected. Until a registry is
//! saved, `load_registry` derives one from legacy settings without writing.
use crate::providers::service_registry::{
    migrate_legacy, validate_snapshot, Purpose, RegistrySnapshot,
};
use crate::{database_error, now_iso};
use rusqlite::{params, Connection, OptionalExtension};

const NAMESPACE: &str = "providers.registry";
const KEY: &str = "default";
const REGISTRY_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone)]
pub(crate) struct LoadedRegistry {
    pub(crate) snapshot: RegistrySnapshot,
    /// Value of `settings_revision` the snapshot was read at.
    pub(crate) revision: i64,
    /// False while the snapshot is derived from legacy settings and unsaved.
    pub(crate) persisted: bool,
}

pub(crate) fn settings_revision(connection: &Connection) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT revision FROM settings_revision WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)
}

fn derive_from_legacy(connection: &Connection) -> Result<RegistrySnapshot, String> {
    let providers = super::settings::load_model_providers(connection)?;
    let routing = super::settings::load_routing_settings(connection)?;
    migrate_legacy(&providers, &routing)
}

/// The legacy provider list and voice routes stay authoritative for what they
/// own; the stored registry contributes the services created through it and the
/// bindings the legacy settings cannot express. A binding whose resource no
/// longer exists becomes unconfigured rather than switching to another service.
fn overlay_legacy(stored: RegistrySnapshot, derived: RegistrySnapshot) -> RegistrySnapshot {
    let mut merged = derived.clone();
    merged.connections.extend(
        stored
            .connections
            .into_iter()
            .filter(|item| item.connection_id.starts_with("conn:svc-")),
    );
    merged.resources.extend(
        stored
            .resources
            .into_iter()
            .filter(|item| item.resource_id.starts_with("res:svc-")),
    );
    let stored_bindings = stored.bindings;
    for binding in &mut merged.bindings {
        let is_voice = matches!(
            binding.purpose,
            Purpose::VoiceTranscribe | Purpose::VoiceSpeak
        );
        if is_voice
            && !stored_bindings
                .iter()
                .find(|item| item.purpose == binding.purpose)
                .and_then(|b| b.primary_resource_id.as_deref())
                .is_some_and(|id| id.starts_with("res:svc-"))
        {
            if let Some(saved) = stored_bindings
                .iter()
                .find(|item| item.purpose == binding.purpose)
            {
                binding.cloud_allowed = saved.cloud_allowed;
            }
            continue;
        }
        if let Some(saved) = stored_bindings
            .iter()
            .find(|item| item.purpose == binding.purpose)
        {
            *binding = saved.clone();
        }
    }
    let known: std::collections::HashSet<String> = merged
        .resources
        .iter()
        .map(|resource| resource.resource_id.clone())
        .collect();
    for binding in &mut merged.bindings {
        let missing = binding
            .primary_resource_id
            .as_ref()
            .is_some_and(|id| !known.contains(id));
        if missing {
            binding.enabled = false;
            binding.primary_resource_id = None;
            binding.fallback_resource_ids.clear();
        }
        binding
            .fallback_resource_ids
            .retain(|id| known.contains(id));
        binding
            .stored_primary_resource_id
            .take_if(|id| !known.contains(id));
    }
    merged
}

pub(crate) fn load_registry(connection: &Connection) -> Result<LoadedRegistry, String> {
    let revision = settings_revision(connection)?;
    let stored: Option<String> = connection
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace=?1 AND key=?2",
            params![NAMESPACE, KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    if let Some(text) = stored {
        let snapshot: RegistrySnapshot = serde_json::from_str(&text)
            .map_err(|error| format!("Could not decode the service registry: {error}"))?;
        let snapshot = overlay_legacy(snapshot, derive_from_legacy(connection)?);
        validate_snapshot(&snapshot)?;
        return Ok(LoadedRegistry {
            snapshot,
            revision,
            persisted: true,
        });
    }
    Ok(LoadedRegistry {
        snapshot: derive_from_legacy(connection)?,
        revision,
        persisted: false,
    })
}

/// Validates and stores the registry. Fails without writing when another
/// settings change landed after `expected_revision` was read.
pub(crate) fn save_registry(
    connection: &mut Connection,
    snapshot: &RegistrySnapshot,
    expected_revision: i64,
) -> Result<LoadedRegistry, String> {
    validate_snapshot(snapshot)?;
    let text = serde_json::to_string(snapshot)
        .map_err(|error| format!("Could not encode the service registry: {error}"))?;
    let transaction = connection.transaction().map_err(database_error)?;
    if settings_revision(&transaction)? != expected_revision {
        return Err("Settings changed since they were loaded; reload and review".to_string());
    }
    reject_cloud_while_local_only(&transaction, snapshot)?;
    validate_owned_ids(&transaction, snapshot)?;
    super::settings::registry_projection::project_voice_bindings(&transaction, snapshot)?;
    transaction
        .execute(
            "INSERT INTO settings_documents(namespace, key, schema_version, value_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(namespace, key) DO UPDATE SET
               schema_version = excluded.schema_version,
               value_json = excluded.value_json,
               updated_at = excluded.updated_at",
            params![NAMESPACE, KEY, REGISTRY_SCHEMA_VERSION, text, now_iso()],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    load_registry(connection)
}

/// A successful save must never silently discard a caller-created resource.
fn validate_owned_ids(connection: &Connection, snapshot: &RegistrySnapshot) -> Result<(), String> {
    let legacy = derive_from_legacy(connection)?;
    for item in &snapshot.connections {
        if !item.connection_id.starts_with("conn:svc-") {
            let original = legacy
                .connection(&item.connection_id)
                .ok_or("New connection IDs must start with conn:svc-")?;
            if serde_json::to_value(original).map_err(|e| e.to_string())?
                != serde_json::to_value(item).map_err(|e| e.to_string())?
            {
                return Err("従来のサービスの変更は既存のサービス設定から保存してください".into());
            }
        }
    }
    for item in &snapshot.resources {
        if !item.resource_id.starts_with("res:svc-") {
            let original = legacy
                .resource(&item.resource_id)
                .ok_or("New resource IDs must start with res:svc-")?;
            if serde_json::to_value(original).map_err(|e| e.to_string())?
                != serde_json::to_value(item).map_err(|e| e.to_string())?
            {
                return Err("従来のモデルの変更は既存のサービス設定から保存してください".into());
            }
        }
    }
    Ok(())
}

/// Mirrors the legacy rule: while the local-only policy is on, a local primary
/// must not fall back to a cloud service.
fn reject_cloud_while_local_only(
    connection: &Connection,
    snapshot: &RegistrySnapshot,
) -> Result<(), String> {
    let document =
        super::settings::read_settings_document(connection, "security.runtime", "default")?;
    let local_only = document
        .value_json
        .get("localOnlyWhenSelected")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if !local_only {
        return Ok(());
    }
    let location = |resource_id: &str| {
        snapshot
            .resource(resource_id)
            .and_then(|resource| snapshot.connection(&resource.connection_id))
            .map(|connection| connection.location.as_str())
    };
    for binding in snapshot.bindings.iter().filter(|binding| {
        binding.enabled
            && binding.review == crate::providers::service_registry::BindingReview::Ready
    }) {
        let primary_is_local = binding
            .primary_resource_id
            .as_deref()
            .is_some_and(|id| location(id) == Some("local"));
        if let Some(id) = binding
            .fallback_resource_ids
            .iter()
            .find(|id| primary_is_local && location(id) == Some("cloud"))
        {
            return Err(format!(
                "Cloud fallback is blocked while the local-only policy is active: {id}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::initialize_database;
    use crate::providers::service_registry::{BindingReview, Purpose};

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
    }

    #[test]
    fn unsaved_registry_is_derived_from_legacy_settings_without_writing() {
        let connection = database();
        let before = settings_revision(&connection).unwrap();
        let loaded = load_registry(&connection).unwrap();
        assert!(!loaded.persisted);
        assert_eq!(settings_revision(&connection).unwrap(), before);
        assert!(loaded
            .snapshot
            .binding(Purpose::ConversationRespond)
            .is_some());
    }

    #[test]
    fn saved_registry_round_trips_and_legacy_settings_are_untouched() {
        let mut connection = database();
        let loaded = load_registry(&connection).unwrap();
        let legacy_before = crate::persistence::settings::list_settings_documents(&connection)
            .unwrap()
            .len();
        let mut snapshot = loaded.snapshot.clone();
        snapshot.bindings[0].review = BindingReview::NeedsReview;
        let saved = save_registry(&mut connection, &snapshot, loaded.revision).unwrap();
        assert!(saved.persisted);
        assert_eq!(
            saved.snapshot.bindings[0].review,
            BindingReview::NeedsReview
        );
        assert_eq!(
            crate::persistence::settings::list_settings_documents(&connection)
                .unwrap()
                .len(),
            legacy_before
        );
    }

    #[test]
    fn local_only_policy_blocks_a_cloud_fallback_behind_a_local_primary() {
        let mut connection = database();
        let loaded = load_registry(&connection).unwrap();
        let mut snapshot = loaded.snapshot.clone();
        snapshot
            .connections
            .push(crate::providers::service_registry::ServiceConnection {
                connection_id: "conn:c".into(),
                label: "c".into(),
                adapter_kind: crate::providers::service_registry::AdapterKind::HttpAsr,
                endpoint: "https://api.example.test/v1".into(),
                location: "cloud".into(),
                authentication: "none".into(),
                credential_ref: None,
                enabled: true,
            });
        snapshot
            .resources
            .push(crate::providers::service_registry::ServiceResource {
                resource_id: "res:c".into(),
                connection_id: "conn:c".into(),
                capability: crate::providers::service_registry::Capability::Transcription,
                model: "m".into(),
                detail: None,
                request_options: None,
                enabled: true,
            });
        let asr = snapshot
            .bindings
            .iter_mut()
            .find(|b| b.purpose == Purpose::VoiceTranscribe)
            .unwrap();
        asr.primary_resource_id = Some("res:harness-asr".into());
        asr.fallback_resource_ids = vec!["res:c".into()];
        asr.enabled = true;
        let error = save_registry(&mut connection, &snapshot, loaded.revision).unwrap_err();
        assert!(error.contains("local-only"));
    }

    fn add_local_asr(connection: &Connection) {
        let mut providers: serde_json::Value = serde_json::from_str(
            &connection
                .query_row(
                    "SELECT value_json FROM settings_documents WHERE namespace='providers.model'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        providers["providers"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "kind": "cloud-asr", "id": "local-asr", "enabled": true, "label": "Local ASR",
                "location": "local", "endpoint": "http://127.0.0.1:9000/v1", "model": "w",
                "language": "auto", "authentication": "none"
            }));
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.model'",
                [providers.to_string()],
            )
            .unwrap();
    }

    #[test]
    fn voice_selection_is_projected_to_the_legacy_route_and_reads_back_from_it() {
        let mut connection = database();
        add_local_asr(&connection);
        let loaded = load_registry(&connection).unwrap();
        let snapshot = {
            let mut next = loaded.snapshot.clone();
            let binding = next
                .bindings
                .iter_mut()
                .find(|b| b.purpose == Purpose::VoiceTranscribe)
                .unwrap();
            binding.primary_resource_id = Some("res:local-asr".into());
            next
        };
        save_registry(&mut connection, &snapshot, loaded.revision).unwrap();
        let routing = crate::persistence::settings::load_routing_settings(&connection).unwrap();
        assert_eq!(routing.voice_transcribe.source, "provider");
        assert_eq!(
            routing.voice_transcribe.provider_id.as_deref(),
            Some("local-asr")
        );
        // A later legacy edit is what the registry shows: one value, not two.
        let mut legacy: serde_json::Value = serde_json::to_value(&routing).unwrap();
        legacy["voiceTranscribe"]["source"] = "harness".into();
        legacy["voiceTranscribe"]["providerId"] = serde_json::Value::Null;
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1 WHERE namespace='routing.tasks'",
                [legacy.to_string()],
            )
            .unwrap();
        let reloaded = load_registry(&connection).unwrap();
        let asr = reloaded.snapshot.binding(Purpose::VoiceTranscribe).unwrap();
        assert_eq!(asr.primary_resource_id.as_deref(), Some("res:harness-asr"));
    }

    #[test]
    fn voice_cannot_be_unselected_or_use_the_harness_as_a_fallback() {
        let mut connection = database();
        let loaded = load_registry(&connection).unwrap();
        let mut off = loaded.snapshot.clone();
        let binding = off
            .bindings
            .iter_mut()
            .find(|b| b.purpose == Purpose::VoiceSpeak)
            .unwrap();
        binding.enabled = false;
        binding.primary_resource_id = None;
        assert!(save_registry(&mut connection, &off, loaded.revision).is_err());
        let mut harness_fallback = loaded.snapshot.clone();
        let binding = harness_fallback
            .bindings
            .iter_mut()
            .find(|b| b.purpose == Purpose::VoiceTranscribe)
            .unwrap();
        binding.fallback_resource_ids = vec!["res:harness-asr".into()];
        assert!(save_registry(&mut connection, &harness_fallback, loaded.revision).is_err());
        assert!(!load_registry(&connection).unwrap().persisted);
    }

    #[test]
    fn stale_revision_is_a_conflict_and_invalid_snapshots_are_not_stored() {
        let mut connection = database();
        let loaded = load_registry(&connection).unwrap();
        save_registry(&mut connection, &loaded.snapshot, loaded.revision).unwrap();
        let conflict = save_registry(&mut connection, &loaded.snapshot, loaded.revision);
        assert!(conflict.unwrap_err().contains("reload"));
        let fresh = load_registry(&connection).unwrap();
        let mut broken = fresh.snapshot.clone();
        broken.bindings[1].primary_resource_id = Some("res:missing".into());
        assert!(save_registry(&mut connection, &broken, fresh.revision).is_err());
        assert!(load_registry(&connection).unwrap().persisted);
    }
}

#[cfg(test)]
mod registry_id_regressions {
    use super::*;
    use crate::providers::service_registry::*;
    #[test]
    fn an_unowned_new_id_is_rejected_before_commit() {
        let mut db = Connection::open_in_memory().unwrap();
        crate::initialize_database(&db).unwrap();
        let loaded = load_registry(&db).unwrap();
        let mut snapshot = loaded.snapshot;
        snapshot.connections.push(ServiceConnection {
            connection_id: "conn:other".into(),
            label: "Other".into(),
            adapter_kind: AdapterKind::ChatCompletions,
            endpoint: "http://127.0.0.1:9000/v1".into(),
            location: "cloud".into(),
            authentication: "none".into(),
            credential_ref: None,
            enabled: false,
        });
        assert!(save_registry(&mut db, &snapshot, loaded.revision)
            .unwrap_err()
            .contains("conn:svc-"));
        assert_eq!(settings_revision(&db).unwrap(), loaded.revision);
        assert!(!load_registry(&db).unwrap().persisted);
        snapshot.connections.last_mut().unwrap().connection_id = "conn:svc-other".into();
        let saved = save_registry(&mut db, &snapshot, loaded.revision).unwrap();
        assert!(saved.snapshot.connection("conn:svc-other").is_some());
        let reloaded = load_registry(&db).unwrap();
        assert_eq!(
            serde_json::to_value(saved.snapshot).unwrap(),
            serde_json::to_value(reloaded.snapshot).unwrap()
        );
    }
}
