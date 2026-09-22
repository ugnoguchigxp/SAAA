use super::settings::default_settings_documents;
use super::settings::SETTINGS_SCHEMA_VERSION;
use crate::backup::backup_connection_to;
use crate::{
    database_error, now_iso, providers, situation, voice, DEFAULT_DYNAMIC_LAN_HOST,
    DYNAMIC_LAN_PROVIDER_ID,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
include!("migrate.d/01.rs");
include!("migrate.d/02.rs");
#[cfg(test)]
mod tests {
    include!("migrate.d/03.rs");
    include!("migrate.d/04.rs");
    include!("migrate.d/05.rs");
}
