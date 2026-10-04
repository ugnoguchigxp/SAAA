mod voice_fallbacks;
use rusqlite::{params, Connection};
mod provider_validation;
pub(crate) mod regional_preferences;
#[path = "settings_defaults.rs"]
mod settings_defaults;
use crate::{
    database_error, now_iso, providers, situation, RoutingSettings, SaveSettingsDocumentInput,
    SecurityRuntimeSettings, SettingsDocument, VoiceRuntimeSettings,
};
#[cfg(any(test, feature = "offline-contracts"))]
use crate::{CodexAgentRuntimeSettings, ModelProviderSettings, ModelProvidersSettings};
pub(crate) use provider_validation::validate_model_providers;
pub(crate) use settings_defaults::default_settings_documents;
mod documents;
pub(crate) mod registry_projection;
pub(crate) use documents::*;
#[cfg(test)]
#[path = "settings/document_tests.rs"]
mod tests;
#[path = "settings/validation.rs"]
mod validation;
pub(crate) use validation::*;
