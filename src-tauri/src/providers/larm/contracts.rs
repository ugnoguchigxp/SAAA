const IDENTIFIER_MAX_BYTES: usize = 160;

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
    ClientDisconnected,
    Cancelled,
    PartialOutput,
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

    pub(crate) fn permits_stream_renew_retry(self) -> bool {
        matches!(self, Self::Network | Self::Timeout | Self::Upstream)
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
    pub(crate) binding_fingerprint: BoundedIdentifier,
    pub(crate) effective_ttl_seconds: u32,
    pub(crate) fallback_used: bool,
    pub(crate) selection_reason: SelectionReason,
}

impl ReadyAllocation {
    pub(crate) fn new(
        allocation_id: impl Into<String>,
        selected_runtime_id: impl Into<String>,
        binding_fingerprint: impl Into<String>,
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
        Ok(Self {
            allocation_id: BoundedIdentifier::new(allocation_id)?,
            selected_runtime_id: BoundedIdentifier::new(selected_runtime_id)?,
            binding_fingerprint: BoundedIdentifier::new(binding_fingerprint)?,
            effective_ttl_seconds,
            fallback_used,
            selection_reason,
        })
    }

    pub(crate) fn same_binding_as(&self, other: &Self) -> bool {
        self.allocation_id == other.allocation_id
            && self.selected_runtime_id == other.selected_runtime_id
            && self.binding_fingerprint == other.binding_fingerprint
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
    InvalidEffectiveTtl,
    FallbackNotAllowed,
}
