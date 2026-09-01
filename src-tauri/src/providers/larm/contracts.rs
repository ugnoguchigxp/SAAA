use serde::Deserialize;

const IDENTIFIER_MAX_BYTES: usize = 160;
const BINDING_IDENTITY_MAX_BYTES: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoundedIdentifier(String);

impl BoundedIdentifier {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > IDENTIFIER_MAX_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ContractError::InvalidIdentifier);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AllocationStatus {
    Pending,
    Ready,
    Failed,
    Released,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AllocationRequirementDto {
    pub(crate) capability: String,
    pub(crate) route: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AllocationBindingDto {
    pub(crate) capability: String,
    pub(crate) route: String,
    pub(crate) runtime: String,
    pub(crate) node: String,
    pub(crate) status: String,
    pub(crate) candidate_rank: u32,
    pub(crate) fallback: bool,
    pub(crate) selection_reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct AllocationErrorDto {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AllocationDto {
    pub(crate) id: String,
    pub(crate) client: Option<String>,
    pub(crate) status: AllocationStatus,
    pub(crate) requirements: Vec<AllocationRequirementDto>,
    pub(crate) bindings: Vec<AllocationBindingDto>,
    pub(crate) allow_fallback: bool,
    pub(crate) deployment_policy: String,
    pub(crate) created_at: String,
    pub(crate) expires_at: String,
    pub(crate) operation_id: Option<String>,
    pub(crate) released_at: Option<String>,
    pub(crate) error: Option<AllocationErrorDto>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperationDto {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) allocation_id: Option<String>,
    pub(crate) status: OperationStatus,
    pub(crate) ready: bool,
    pub(crate) desired: Vec<String>,
    pub(crate) ensure: Vec<String>,
    pub(crate) created_at: String,
    pub(crate) deadline_at: Option<String>,
    pub(crate) completed_at: Option<String>,
    pub(crate) phase: Option<String>,
    pub(crate) error: Option<AllocationErrorDto>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionReason {
    Primary,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionFailureKind {
    Authentication,
    Contract,
    Protocol,
    RequestTooLarge,
    Internal,
    Cancelled,
    Policy,
    Capacity,
    Unavailable,
    Draining,
    Upstream,
    Network,
    Timeout,
    AllocationLost,
    AllocationOutcomeUnknown,
    NotReady,
}

impl SessionFailureKind {
    pub(crate) fn permits_pre_stream_reallocation(self) -> bool {
        matches!(
            self,
            Self::Network | Self::Timeout | Self::Upstream | Self::AllocationLost
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReleaseFailureKind {
    Authentication,
    Protocol,
    Upstream,
    Network,
    Timeout,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyAllocation {
    pub(crate) allocation_id: BoundedIdentifier,
    pub(crate) selected_runtime_id: BoundedIdentifier,
    binding_identity: String,
    pub(crate) effective_ttl_seconds: u32,
    pub(crate) fallback_used: bool,
    pub(crate) selection_reason: SelectionReason,
}

impl ReadyAllocation {
    pub(crate) fn new_with_binding_identity(
        allocation_id: impl Into<String>,
        selected_runtime_id: impl Into<String>,
        binding_identity: impl Into<String>,
        effective_ttl_seconds: u32,
        fallback_used: bool,
        selection_reason: SelectionReason,
    ) -> Result<Self, ContractError> {
        if !(60..=3_600).contains(&effective_ttl_seconds) {
            return Err(ContractError::InvalidEffectiveTtl);
        }
        if fallback_used {
            return Err(ContractError::FallbackNotAllowed);
        }
        let binding_identity = binding_identity.into();
        if binding_identity.is_empty()
            || binding_identity.len() > BINDING_IDENTITY_MAX_BYTES
            || binding_identity.chars().any(char::is_control)
        {
            return Err(ContractError::InvalidBindingIdentity);
        }
        Ok(Self {
            allocation_id: BoundedIdentifier::new(allocation_id)?,
            selected_runtime_id: BoundedIdentifier::new(selected_runtime_id)?,
            binding_identity,
            effective_ttl_seconds,
            fallback_used,
            selection_reason,
        })
    }

    #[cfg(test)]
    pub(crate) fn same_binding_as(&self, other: &Self) -> bool {
        self.allocation_id == other.allocation_id
            && self.selected_runtime_id == other.selected_runtime_id
            && self.binding_identity == other.binding_identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingAllocation {
    pub(crate) operation_id: BoundedIdentifier,
    pub(crate) cleanup_allocation_id: Option<BoundedIdentifier>,
}

impl PendingAllocation {
    pub(crate) fn new(
        operation_id: impl Into<String>,
        cleanup_allocation_id: Option<String>,
    ) -> Result<Self, ContractError> {
        Ok(Self {
            operation_id: BoundedIdentifier::new(operation_id)?,
            cleanup_allocation_id: cleanup_allocation_id
                .map(BoundedIdentifier::new)
                .transpose()?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContractError {
    InvalidIdentifier,
    InvalidBindingIdentity,
    InvalidEffectiveTtl,
    FallbackNotAllowed,
}
