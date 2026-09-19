use crate::{
    persistence::{settings::default_settings_documents, SqliteWriter},
    AppState, DynamicLanProviderSettings, ModelProviderSettings, OpenAiCompatibleProviderSettings,
    SaveSettingsDocumentInput,
};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) fn app_state(connection: Connection) -> AppState {
    let sqlite_writer = Arc::new(SqliteWriter::from_connection(connection));
    let generated_capabilities = Arc::new(
        crate::generated_capabilities::service::CapabilityService::build(
            sqlite_writer.clone(),
            &PathBuf::new(),
            PathBuf::new(),
            None,
        ),
    );
    crate::test_state::app_state_with_capabilities(sqlite_writer, generated_capabilities)
}

pub(crate) fn provider(id: &str, location: &str) -> ModelProviderSettings {
    ModelProviderSettings::OpenAiCompatible(direct_provider(id, location))
}

pub(crate) fn direct_provider(id: &str, location: &str) -> OpenAiCompatibleProviderSettings {
    OpenAiCompatibleProviderSettings {
        request_options: None,
        id: id.to_string(),
        enabled: true,
        label: id.to_string(),
        location: location.to_string(),
        endpoint: if location == "local" {
            "http://127.0.0.1:11434/v1".to_string()
        } else {
            "https://example.invalid/v1".to_string()
        },
        model: "test-model".to_string(),
        authentication: "none".to_string(),
    }
}

pub(crate) fn dynamic_lan_provider(id: &str) -> ModelProviderSettings {
    ModelProviderSettings::DynamicLan(DynamicLanProviderSettings {
        request_options: None,
        id: id.to_string(),
        enabled: true,
        label: id.to_string(),
        location: "local".to_string(),
        host: "10.0.0.42".to_string(),
    })
}

pub(crate) fn default_settings_input() -> Vec<SaveSettingsDocumentInput> {
    default_settings_documents()
        .into_iter()
        .map(
            |(namespace, key, schema_version, value_json)| SaveSettingsDocumentInput {
                namespace: namespace.to_string(),
                key: key.to_string(),
                schema_version,
                value_json,
            },
        )
        .collect()
}
