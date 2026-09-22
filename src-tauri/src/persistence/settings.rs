mod voice_fallbacks;
use rusqlite::{params, Connection};
mod provider_validation;
pub(crate) mod regional_preferences;
#[path = "settings_defaults.rs"]
mod settings_defaults;
use crate::{
    database_error, now_iso, providers, situation, CodexAgentRuntimeSettings,
    ModelProviderSettings, ModelProvidersSettings, RoutingSettings, SaveSettingsDocumentInput,
    SecurityRuntimeSettings, SettingsDocument, VoiceRuntimeSettings,
};
pub(crate) use provider_validation::validate_model_providers;
pub(crate) use settings_defaults::default_settings_documents;
include!("settings.d/01.rs");
include!("settings.d/02.rs");
include!("settings.d/03.rs");
