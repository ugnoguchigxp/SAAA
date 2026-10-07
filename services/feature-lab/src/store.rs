//! One lab database owner. Initialization failure keeps the file.
use rusqlite::{params, Connection};

use crate::owner::{self, OwnerGuard};
use saaa_provider_routing::{
    BindingReview, Capability, Purpose, PurposeBinding, RegistrySnapshot, ServiceConnection,
    ServiceResource,
};
use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabConfig {
    pub database_path: String,
    pub allowed_origin: String,
    pub session_token: String,
    pub provider: String,
    pub larm_endpoint: String,
    pub larm_token: Option<String>,
    #[serde(default)]
    pub replace_route: bool,
}

pub struct LabOpen {
    pub connection: Connection,
    pub config: LabConfig,
    pub owner: OwnerGuard,
}

const IDENTITY: &str = "feature-lab";

fn session_token_ok(token: &str) -> bool {
    (32..=256).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn open_lab(config: LabConfig) -> Result<LabOpen, String> {
    if !session_token_ok(&config.session_token) {
        return Err("lab session token is invalid".into());
    }
    let path = Path::new(&config.database_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let existed = path.exists();
    let owner = owner::acquire(path)?;
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    if existed && !identity_ok(&connection)? {
        return Err("refusing a database that is not a feature lab".into());
    }
    saaa_provider_routing::initialize_settings_documents(&connection)?;
    saaa_provider_routing::initialize_audit(&connection)?;
    saaa_media::initialize(&connection)?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS lab_identity(
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                kind TEXT NOT NULL CHECK(kind = 'feature-lab')
            );",
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT OR IGNORE INTO lab_identity(singleton, kind) VALUES(1, ?1)",
            [IDENTITY],
        )
        .map_err(|error| error.to_string())?;
    align_image_route(&connection, &config)?;
    Ok(LabOpen {
        connection,
        config,
        owner,
    })
}

fn identity_ok(connection: &Connection) -> Result<bool, String> {
    let names = table_names(connection)?;
    if names.is_empty() {
        return Ok(true);
    }
    if !names.iter().any(|name| name == "lab_identity") {
        return Ok(false);
    }
    let kind: String = connection
        .query_row(
            "SELECT kind FROM lab_identity WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(kind == IDENTITY)
}

fn table_names(connection: &Connection) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn align_image_route(connection: &Connection, config: &LabConfig) -> Result<(), String> {
    let existing: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM settings_documents WHERE namespace = 'providers.registry' AND key = 'default'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if existing > 0 {
        return match_saved_endpoint(connection, &config.larm_endpoint, config.replace_route);
    }
    let endpoint = &config.larm_endpoint;
    let snapshot = RegistrySnapshot {
        connections: vec![ServiceConnection {
            connection_id: "conn:harness".into(),
            label: "Lab LARM".into(),
            adapter_kind: saaa_provider_routing::AdapterKind::Larm,
            endpoint: endpoint.into(),
            location: "local".into(),
            authentication: "none".into(),
            credential_ref: None,
            enabled: true,
        }],
        resources: vec![ServiceResource {
            resource_id: "res:lab-image".into(),
            connection_id: "conn:harness".into(),
            capability: Capability::ImageGeneration,
            model: "lab-image".into(),
            detail: None,
            request_options: None,
            enabled: true,
        }],
        bindings: vec![PurposeBinding {
            purpose: Purpose::MediaImageGenerate,
            enabled: true,
            primary_resource_id: Some("res:lab-image".into()),
            fallback_resource_ids: Vec::new(),
            cloud_allowed: false,
            timeout_ms: 30_000,
            attempt_timeout_ms: Some(30_000),
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        }],
    };
    saaa_provider_routing::validate_snapshot(&snapshot)?;
    let text = serde_json::to_string(&snapshot).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO settings_documents(namespace, key, schema_version, value_json, updated_at)
             VALUES('providers.registry', 'default', 1, ?1, ?2)",
            params![text, "0"],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn match_saved_endpoint(
    connection: &Connection,
    endpoint: &str,
    replace: bool,
) -> Result<(), String> {
    let mut snapshot = load_snapshot(connection)?;
    let Some(index) = snapshot
        .connections
        .iter()
        .position(|item| item.connection_id == "conn:harness")
    else {
        return Err("lab route is missing the saved connection".into());
    };
    if snapshot.connections[index].endpoint == endpoint {
        return Ok(());
    }
    if !replace {
        return Err(format!(
            "lab route endpoint is {}, refusing {}. Pass replaceRoute to switch the saved route.",
            snapshot.connections[index].endpoint, endpoint
        ));
    }
    snapshot.connections[index].endpoint = endpoint.to_string();
    saaa_provider_routing::validate_snapshot(&snapshot)?;
    let text = serde_json::to_string(&snapshot).map_err(|error| error.to_string())?;
    let count = connection
        .execute(
            "UPDATE settings_documents SET value_json=?1, updated_at=?2 WHERE namespace='providers.registry' AND key='default'",
            params![text, "0"],
        )
        .map_err(|error| error.to_string())?;
    if count != 1 {
        return Err("lab route could not be switched".into());
    }
    Ok(())
}

pub fn load_snapshot(connection: &Connection) -> Result<RegistrySnapshot, String> {
    let text: String = connection
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace = 'providers.registry' AND key = 'default'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}
