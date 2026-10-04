use super::types::*;
use sha2::{Digest, Sha256};

/// Route fixed at job start. Later settings edits do not change it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedRoute {
    pub(crate) purpose: Purpose,
    pub(crate) connection_id: String,
    pub(crate) resource_id: String,
    pub(crate) adapter_kind: AdapterKind,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) credential_ref: Option<CredentialRef>,
    pub(crate) fallback_resource_ids: Vec<String>,
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
}

pub(crate) fn resolve_route(
    snapshot: &RegistrySnapshot,
    purpose: Purpose,
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
    let digest = Sha256::digest(
        serde_json::to_vec(&(connection, resource, binding))
            .map_err(|error| ResolveError::Invalid(error.to_string()))?,
    );
    Ok(ResolvedRoute {
        purpose,
        connection_id: connection.connection_id.clone(),
        resource_id: resource.resource_id.clone(),
        adapter_kind: connection.adapter_kind,
        endpoint: connection.endpoint.clone(),
        model: resource.model.clone(),
        credential_ref: connection.credential_ref.clone(),
        fallback_resource_ids: binding.fallback_resource_ids.clone(),
        timeout_ms: binding.timeout_ms,
        attempt_timeout_ms: binding.attempt_timeout_ms,
        fingerprint: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}
