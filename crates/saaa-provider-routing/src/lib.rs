//! Purpose-route selection shared by desktop and the media host.
//! Hosts collect availability and legacy settings, then pass values in.
//! This crate does not open a database file or depend on Tauri.

mod active;
mod audit;
mod compatibility;
mod migration;
mod resolve;
mod schema;
mod types;
mod validate;

#[cfg(test)]
mod tests;

pub use active::{route_still_active, validate_active};
pub use audit::{accepted, attempt, attributes};
pub use compatibility::unsupported_reason;
pub use migration::{migrate_legacy, LegacyHarness, LegacyProvider, LegacyRoute, LegacySettings};
pub use resolve::{
    resolve_resource, resolve_route, LarmReachability, ResolveError, ResolvedRoute, RouteSelection,
};
pub use schema::{
    initialize_audit, initialize_credential_secrets, initialize_settings_documents,
    read_named_secret,
};
pub use types::*;
pub use validate::validate_snapshot;

pub const PROVIDER_CREDENTIAL_SERVICE: &str = "com.saaa.provider-api-key";
pub const SERVICE_CONNECTION_CREDENTIAL_SERVICE: &str = "com.saaa.service-connection";

pub fn database_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

/// `owner/name` or `owner/name:` plus a 64-digit version id.
pub fn model_parts(model: &str) -> Result<(&str, Option<&str>), String> {
    let (name, version) = model
        .split_once(':')
        .map_or((model, None), |(name, version)| (name, Some(version)));
    let parts = name.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 80
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
        || version
            .is_some_and(|value| value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(
            "モデルはowner/name、版を指定する場合はowner/name:64桁の版IDにしてください".into(),
        );
    }
    Ok((name, version))
}
