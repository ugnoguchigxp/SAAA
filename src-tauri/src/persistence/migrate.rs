use super::settings::default_settings_documents;
use super::settings::SETTINGS_SCHEMA_VERSION;
use crate::backup::backup_connection_to;
use crate::{
    database_error, now_iso, providers, situation, voice, DEFAULT_DYNAMIC_LAN_HOST,
};
pub(super) use crate::DYNAMIC_LAN_PROVIDER_ID;
pub(super) use rusqlite::{params, Connection, OptionalExtension};
pub(super) use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
#[path = "migrate/ensure_provider_configuration_fingerprint.rs"]
mod ensure_provider_configuration_fingerprint;
#[path = "migrate/migrate_pristine_provider_defaults_to_dynamic_la.rs"]
mod migrate_pristine_provider_defaults_to_dynamic_la;
pub(crate) use ensure_provider_configuration_fingerprint::{ensure_provider_configuration_fingerprint, migrate_provider_reasoning_effort_default, migrate_direct_dynamic_lan_provider_to_discovery, migrate_v4_to_v5, migrate_v6_to_v7, settings_template_for_v7, settings_template_for_legacy_v8_or_v9, migrate_v7_to_v8, migrate_v8_to_v9, normalize_json_to_template, migrate_v26_to_v27, backup_before_migration};
pub(crate) use migrate_pristine_provider_defaults_to_dynamic_la::{migrate_pristine_provider_defaults_to_dynamic_lan, migrate_legacy_settings_documents, migrate_document};
#[cfg(test)]
pub(super) use crate::initialize_database;
#[cfg(test)]
pub(super) use crate::persistence::list_settings_documents;
#[cfg(test)]
pub(super) use crate::persistence::provider_identity::migrate_dynamic_lan_provider_identity;

#[cfg(test)]
#[path = "migrate/tests/mod.rs"]
mod tests;
