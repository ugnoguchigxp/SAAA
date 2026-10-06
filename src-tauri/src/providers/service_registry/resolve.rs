use super::types::*;
use crate::providers::reachability::{Reachability, ReachabilitySnapshot};
use sha2::{Digest, Sha256};

/// Why a route was chosen. Recorded in the audit trail and shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RouteSelection {
    /// The binding's primary resource.
    #[default]
    Primary,
    /// A fallback, chosen because LARM was unreachable.
    LocalUnreachable,
}

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

/// Route fixed at job start. Later settings edits do not change it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolvedRoute {
    pub(crate) purpose: Purpose,
    pub(crate) connection_id: String,
    pub(crate) connection_label: String,
    pub(crate) resource_id: String,
    pub(crate) adapter_kind: AdapterKind,
    pub(crate) endpoint: String,
    pub(crate) location: String,
    pub(crate) primary_location: String,
    pub(crate) model: String,
    pub(crate) detail: Option<String>,
    pub(crate) request_options: Option<serde_json::Value>,
    pub(crate) credential_ref: Option<CredentialRef>,
    pub(crate) fallback_resource_ids: Vec<String>,
    #[serde(default)]
    pub(crate) selection: RouteSelection,
    pub(crate) timeout_ms: u64,
    pub(crate) attempt_timeout_ms: Option<u64>,
    /// Hash of the connection, resource and binding settings used by this route.
    pub(crate) fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolveError {
    NotConfigured(Purpose),
    Disabled(Purpose),
    NeedsReview(Purpose),
    ResourceDisabled(String),
    ConnectionDisabled(String),
    Invalid(String),
    CloudNotAllowed(Purpose),
    /// LARM is unreachable and no allowed fallback exists.
    LocalUnreachable {
        purpose: Purpose,
        cloud_blocked: bool,
    },
}

impl ResolveError {
    pub(crate) fn user_message(&self) -> Option<&'static str> {
        match self {
            Self::LocalUnreachable {
                cloud_blocked: false,
                ..
            } => Some("LARMに接続できません。外出時の代替先が設定されていません。"),
            Self::LocalUnreachable {
                cloud_blocked: true,
                ..
            } => {
                Some("LARMに接続できません。代替先へのクラウド送信がこの用途で許可されていません。")
            }
            _ => None,
        }
    }
}

/// The single place that decides which resource serves a request.
/// The primary is used unless it is LARM and LARM is known to be unreachable; then the
/// fallbacks are tried in order. A broken primary is a configuration error, never a switch.
pub(crate) fn resolve_route(
    snapshot: &RegistrySnapshot,
    purpose: Purpose,
    availability: LocalAvailability,
) -> Result<ResolvedRoute, ResolveError> {
    let binding = snapshot
        .binding(purpose)
        .ok_or(ResolveError::NotConfigured(purpose))?;
    if !binding.enabled {
        return Err(ResolveError::Disabled(purpose));
    }
    if binding.review == BindingReview::NeedsReview {
        return Err(ResolveError::NeedsReview(purpose));
    }
    let resource_id = binding
        .primary_resource_id
        .as_deref()
        .ok_or(ResolveError::NotConfigured(purpose))?;
    let primary = resolve_resource(snapshot, purpose, resource_id)?;
    if primary.adapter_kind != AdapterKind::Larm || availability.larm != Reachability::Unreachable {
        return Ok(primary);
    }
    let mut cloud_blocked = false;
    for id in &binding.fallback_resource_ids {
        match resolve_resource(snapshot, purpose, id) {
            Ok(mut route) if route.adapter_kind != AdapterKind::Larm => {
                route.selection = RouteSelection::LocalUnreachable;
                return Ok(route);
            }
            Err(ResolveError::CloudNotAllowed(_)) => cloud_blocked = true,
            _ => {}
        }
    }
    Err(ResolveError::LocalUnreachable {
        purpose,
        cloud_blocked,
    })
}

pub(crate) fn resolve_resource(
    snapshot: &RegistrySnapshot,
    purpose: Purpose,
    resource_id: &str,
) -> Result<ResolvedRoute, ResolveError> {
    let binding = snapshot
        .binding(purpose)
        .ok_or(ResolveError::NotConfigured(purpose))?;
    if !binding.enabled {
        return Err(ResolveError::Disabled(purpose));
    }
    if binding.review != BindingReview::Ready {
        return Err(ResolveError::NeedsReview(purpose));
    }
    let resource = snapshot
        .resource(resource_id)
        .ok_or_else(|| ResolveError::Invalid(format!("unknown resource {resource_id}")))?;
    if resource.capability != purpose.required_capability() {
        return Err(ResolveError::Invalid(format!(
            "resource {resource_id} lacks the required capability"
        )));
    }
    if !resource.enabled {
        return Err(ResolveError::ResourceDisabled(resource.resource_id.clone()));
    }
    let connection = snapshot
        .connection(&resource.connection_id)
        .ok_or_else(|| ResolveError::Invalid("unknown connection".to_string()))?;
    if !connection.enabled {
        return Err(ResolveError::ConnectionDisabled(
            connection.connection_id.clone(),
        ));
    }
    if connection.location == "cloud" && !binding.cloud_allowed {
        return Err(ResolveError::CloudNotAllowed(purpose));
    }
    if let Some(reason) = super::unsupported_reason(snapshot, purpose, resource_id) {
        return Err(ResolveError::Invalid(reason));
    }
    let digest = Sha256::digest(
        serde_json::to_vec(&(connection, resource, binding))
            .map_err(|error| ResolveError::Invalid(error.to_string()))?,
    );
    Ok(ResolvedRoute {
        purpose,
        connection_id: connection.connection_id.clone(),
        connection_label: connection.label.clone(),
        resource_id: resource.resource_id.clone(),
        adapter_kind: connection.adapter_kind,
        endpoint: connection.endpoint.clone(),
        location: connection.location.clone(),
        primary_location: binding
            .primary_resource_id
            .as_deref()
            .and_then(|id| snapshot.resource(id))
            .and_then(|r| snapshot.connection(&r.connection_id))
            .map(|c| c.location.clone())
            .unwrap_or_default(),
        model: resource.model.clone(),
        detail: resource.detail.clone(),
        request_options: resource.request_options.clone(),
        credential_ref: connection.credential_ref.clone(),
        fallback_resource_ids: binding.fallback_resource_ids.clone(),
        selection: RouteSelection::Primary,
        timeout_ms: binding.timeout_ms,
        attempt_timeout_ms: binding.attempt_timeout_ms,
        fingerprint: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}
