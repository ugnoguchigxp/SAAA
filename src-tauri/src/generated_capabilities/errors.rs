use std::{error::Error, fmt};

/// Stable error codes exposed by the generated-capability service. The set is fixed for M1;
/// unknown internal failures must not be converted into success or a boolean `false`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityErrorCode {
    Disabled,
    Unavailable,
    UnsupportedContract,
    UnsupportedPackageLayout,
    InvalidPackage,
    InvalidInput,
    IntegrityError,
    VerificationFailed,
    NotValidated,
    NotActive,
    StaleRevision,
    Conflict,
    Busy,
    Timeout,
    Cancelled,
    OutputLimit,
    ProtocolError,
    StorageError,
}

impl CapabilityErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
            Self::UnsupportedContract => "unsupported-contract",
            Self::UnsupportedPackageLayout => "unsupported-package-layout",
            Self::InvalidPackage => "invalid-package",
            Self::InvalidInput => "invalid-input",
            Self::IntegrityError => "integrity-error",
            Self::VerificationFailed => "verification-failed",
            Self::NotValidated => "not-validated",
            Self::NotActive => "not-active",
            Self::StaleRevision => "stale-revision",
            Self::Conflict => "conflict",
            Self::Busy => "busy",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::OutputLimit => "output-limit",
            Self::ProtocolError => "protocol-error",
            Self::StorageError => "storage-error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "disabled" => Self::Disabled,
            "unavailable" => Self::Unavailable,
            "unsupported-contract" => Self::UnsupportedContract,
            "unsupported-package-layout" => Self::UnsupportedPackageLayout,
            "invalid-package" => Self::InvalidPackage,
            "invalid-input" => Self::InvalidInput,
            "integrity-error" => Self::IntegrityError,
            "verification-failed" => Self::VerificationFailed,
            "not-validated" => Self::NotValidated,
            "not-active" => Self::NotActive,
            "stale-revision" => Self::StaleRevision,
            "conflict" => Self::Conflict,
            "busy" => Self::Busy,
            "timeout" => Self::Timeout,
            "cancelled" => Self::Cancelled,
            "output-limit" => Self::OutputLimit,
            "protocol-error" => Self::ProtocolError,
            "storage-error" => Self::StorageError,
            _ => return None,
        })
    }
}

/// A structured failure. Messages stay short and never echo caller input, environment
/// variables, or arbitrary filesystem paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityError {
    pub code: CapabilityErrorCode,
    pub message: String,
}

impl CapabilityError {
    pub fn new(code: CapabilityErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// `SqliteWriter::write` only accepts `Result<_, String>`; this keeps the code attached.
    pub fn encode(&self) -> String {
        format!("{}|{}", self.code.as_str(), self.message)
    }

    pub fn decode(value: String) -> Self {
        match value.split_once('|') {
            Some((code, message)) => match CapabilityErrorCode::parse(code) {
                Some(code) => Self::new(code, message),
                None => Self::new(CapabilityErrorCode::StorageError, value),
            },
            None => Self::new(CapabilityErrorCode::StorageError, value),
        }
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl Error for CapabilityError {}

pub type CapabilityResult<T> = Result<T, CapabilityError>;

pub fn error<T>(code: CapabilityErrorCode, message: impl Into<String>) -> CapabilityResult<T> {
    Err(CapabilityError::new(code, message))
}

#[cfg(test)]
mod tests {
    use super::CapabilityErrorCode;

    #[test]
    fn stable_error_codes_are_spelled_out() {
        let codes = [
            CapabilityErrorCode::Disabled,
            CapabilityErrorCode::Unavailable,
            CapabilityErrorCode::UnsupportedContract,
            CapabilityErrorCode::UnsupportedPackageLayout,
            CapabilityErrorCode::InvalidPackage,
            CapabilityErrorCode::InvalidInput,
            CapabilityErrorCode::IntegrityError,
            CapabilityErrorCode::VerificationFailed,
            CapabilityErrorCode::NotValidated,
            CapabilityErrorCode::NotActive,
            CapabilityErrorCode::StaleRevision,
            CapabilityErrorCode::Conflict,
            CapabilityErrorCode::Busy,
            CapabilityErrorCode::Timeout,
            CapabilityErrorCode::Cancelled,
            CapabilityErrorCode::OutputLimit,
            CapabilityErrorCode::ProtocolError,
            CapabilityErrorCode::StorageError,
        ];
        let rendered = codes.map(CapabilityErrorCode::as_str);
        assert_eq!(rendered[0], "disabled");
        assert_eq!(rendered[17], "storage-error");
        let unique = rendered.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), codes.len());
    }
}
