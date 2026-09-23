#[path = "settings/http_migration.rs"]
mod http_migration;
use super::settings::SETTINGS_SCHEMA_VERSION;
use crate::{
    now_iso, DEFAULT_AGENT_NAME, DEFAULT_DYNAMIC_LAN_HOST, DEFAULT_USER_NAME,
    DYNAMIC_LAN_PROVIDER_ID,
};
use http_migration::migrate_http_bases;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
#[path = "settings_migration/stored_document.rs"]
mod stored_document;
pub(crate) use stored_document::{migrate_settings_to_current};
pub(super) use stored_document::{initialize_revision, migrate_provider_document, migrate_obsolete_direct_lan_route, migrated_voice_document, migrate_security_document};
#[cfg(test)]
#[path = "settings_migration/tests.rs"]
mod tests;
