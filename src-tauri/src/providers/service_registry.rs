//! Purpose-based service registry.
//!
//! Selection, validation and legacy derivation live in `saaa-provider-routing`.
//! This module keeps Tauri commands, probes, and the AppState availability read.
mod active;
pub(crate) mod commands;
pub(crate) use active::validate_active;
mod migration;
pub(crate) mod operations;
mod probe;
#[cfg(test)]
mod tests;

pub(crate) use migration::migrate_legacy;
pub(crate) use saaa_provider_routing::{
    unsupported_reason, validate_snapshot, AdapterKind, BindingReview, Capability, CredentialRef,
    LarmReachability, Purpose, PurposeBinding, RegistrySnapshot, ResolveError, ResolvedRoute,
    RouteSelection, ServiceConnection, ServiceResource,
};
pub(crate) const SERVICE_CREDENTIAL_SERVICE: &str =
    saaa_provider_routing::SERVICE_CONNECTION_CREDENTIAL_SERVICE;

use crate::providers::reachability::{Reachability, ReachabilitySnapshot};

/// Observed state of LARM at request time. `Unknown` is treated as reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LocalAvailability {
    pub(crate) larm: Reachability,
}

impl LocalAvailability {
    pub(crate) fn of(state: &crate::AppState) -> Self {
        Self::from(&state.reachability.snapshot())
    }
}

impl From<&ReachabilitySnapshot> for LocalAvailability {
    fn from(snapshot: &ReachabilitySnapshot) -> Self {
        Self {
            larm: snapshot.harness,
        }
    }
}

fn larm_reachability(availability: LocalAvailability) -> LarmReachability {
    match availability.larm {
        Reachability::Unknown => LarmReachability::Unknown,
        Reachability::Reachable => LarmReachability::Reachable,
        Reachability::Unreachable => LarmReachability::Unreachable,
    }
}

pub(crate) fn resolve_route(
    snapshot: &RegistrySnapshot,
    purpose: Purpose,
    availability: LocalAvailability,
) -> Result<ResolvedRoute, ResolveError> {
    saaa_provider_routing::resolve_route(snapshot, purpose, larm_reachability(availability))
}

pub(crate) fn resolve_resource(
    snapshot: &RegistrySnapshot,
    purpose: Purpose,
    resource_id: &str,
) -> Result<ResolvedRoute, ResolveError> {
    saaa_provider_routing::resolve_resource(snapshot, purpose, resource_id)
}

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
