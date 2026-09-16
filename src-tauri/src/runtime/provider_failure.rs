use super::super::contracts::RunFailureCode;
use super::TurnExecutionFailure;

impl TurnExecutionFailure {
    pub(crate) fn provider(kind: crate::ProviderFailureKind, message: String) -> Self {
        use crate::ProviderFailureKind as Kind;
        let code = match kind {
            Kind::Authentication | Kind::Contract | Kind::Policy => {
                RunFailureCode::ConfigurationError
            }
            Kind::Timeout => RunFailureCode::RequestTimeout,
            Kind::Cancelled => RunFailureCode::UserCancelled,
            Kind::Protocol => RunFailureCode::ProtocolError,
            Kind::RequestTooLarge => RunFailureCode::ResponseTooLarge,
            Kind::Internal => RunFailureCode::InternalError,
            _ => RunFailureCode::ProviderError,
        };
        Self::unsupervised(code, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_categories_survive_the_turn_boundary() {
        use crate::ProviderFailureKind as Kind;
        for (kind, code) in [
            (Kind::Authentication, RunFailureCode::ConfigurationError),
            (Kind::Contract, RunFailureCode::ConfigurationError),
            (Kind::Timeout, RunFailureCode::RequestTimeout),
            (Kind::Cancelled, RunFailureCode::UserCancelled),
            (Kind::Network, RunFailureCode::ProviderError),
        ] {
            let error = TurnExecutionFailure::provider(kind, "same message".into());
            assert_eq!(error.code, code);
        }
    }
}
