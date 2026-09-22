use super::*;
/// Keep distinct context failures actionable without exposing internal source content. These
/// failures happen before a provider request, so retrying with fewer optional items is not a
/// recovery path for required-context overflow.
pub(super) fn context_recovery_message(error: &str) -> String {
    if error.starts_with("required_context_overflow:") {
        return "Required context does not fit this provider. Narrow the task scope, review the original condition, or correct the saved memory before trying again.".into();
    }
    if error.contains("does not belong to the resolved scope")
        || error.contains("context-scope-changed")
        || error.contains("Context scope could not be resolved")
    {
        return "Context scope changed before dispatch. Choose the intended task or scope and try again.".into();
    }
    if error.contains("source is incomplete") || error.contains("source-unavailable") {
        return "A required source is not available yet. Review the original message and try again after it is available.".into();
    }
    error.to_owned()
}

pub(crate) fn provider_fallback_allowed(kind: ProviderFailureKind, output_started: bool) -> bool {
    !output_started
        && matches!(
            kind,
            ProviderFailureKind::Capacity
                | ProviderFailureKind::Unavailable
                | ProviderFailureKind::Upstream
                | ProviderFailureKind::Connect
                | ProviderFailureKind::ResponseInterrupted
                | ProviderFailureKind::Network
                | ProviderFailureKind::Timeout
                | ProviderFailureKind::AllocationLost
        )
}

pub(super) fn provider_route_fallback_allowed(
    _provider: &ModelProviderSettings,
    kind: ProviderFailureKind,
    output_started: bool,
) -> bool {
    provider_fallback_allowed(kind, output_started)
}

#[cfg(test)]
mod tests {
    use super::context_recovery_message;

    #[test]
    fn required_overflow_has_a_specific_non_destructive_recovery() {
        let message =
            context_recovery_message("required_context_overflow: required context exceeds");
        assert!(message.contains("Narrow the task scope"));
        assert!(!message.contains("delete"));
    }

    #[test]
    fn unresolved_scope_has_the_same_specific_recovery() {
        assert!(
            context_recovery_message("Context scope could not be resolved: missing")
                .contains("Choose the intended task or scope")
        );
    }
}
