//! Purpose-based service registry (plan: docs/plans/purpose-based-cloud-api-switching.md, P1).
//!
//! Three layers: connections (where and how to authenticate), resources (what a
//! connection can run) and purpose bindings (which resource does which job).
//! This module is pure data, validation, legacy migration and route resolution.
//! Persistence and the current conversation queue consume the same snapshot.
mod active;
pub(crate) mod commands;
mod compatibility;
pub(crate) use active::validate_active;
mod migration;
pub(crate) mod operations;
mod probe;
mod resolve;
#[cfg(test)]
mod tests;
mod types;
mod validate;

pub(crate) use compatibility::unsupported_reason;
pub(crate) use migration::migrate_legacy;
pub(crate) const SERVICE_CREDENTIAL_SERVICE: &str =
    crate::credentials::SERVICE_CONNECTION_CREDENTIAL_SERVICE;
pub(crate) use resolve::{
    resolve_resource, resolve_route, LocalAvailability, ResolveError, ResolvedRoute, RouteSelection,
};
pub(crate) use types::*;
pub(crate) use validate::validate_snapshot;

pub(crate) fn with_handler<R: tauri::Runtime>(
    fallback: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    let handler: fn(tauri::ipc::Invoke<R>) -> bool = tauri::generate_handler![
        commands::get_service_registry,
        commands::save_service_registry,
        commands::set_service_connection_secret,
        commands::get_service_connection_secret_state,
        commands::probe_service_resource
    ];
    move |invoke: tauri::ipc::Invoke<R>| match invoke.message.command() {
        "get_service_registry"
        | "save_service_registry"
        | "set_service_connection_secret"
        | "get_service_connection_secret_state"
        | "probe_service_resource" => handler(invoke),
        _ => fallback(invoke),
    }
}
