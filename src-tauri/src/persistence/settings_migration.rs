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
include!("settings_migration.d/01.rs");
include!("settings_migration.d/02.rs");
