use super::*;

pub(super) fn result_content(result: CapabilityResult<InvocationResult>) -> String {
    match result {
        Ok(invocation) => json!({
            "ok": true,
            "callId": invocation.call_id,
            "revisionId": invocation.revision_id,
            "value": invocation.value,
        })
        .to_string(),
        Err(error) => error_content(error.code, safe_message(error.code)),
    }
}

pub(super) fn error_content(code: CapabilityErrorCode, message: &str) -> String {
    json!({ "ok": false, "error": { "code": code.as_str(), "message": message } }).to_string()
}

/// Fixed, non-leaking explanations. Internal messages, paths and stderr never reach the model.
pub(super) fn safe_message(code: CapabilityErrorCode) -> &'static str {
    match code {
        CapabilityErrorCode::Disabled | CapabilityErrorCode::Unavailable => {
            "The generated tool is unavailable."
        }
        CapabilityErrorCode::UnsupportedContract
        | CapabilityErrorCode::UnsupportedPackageLayout
        | CapabilityErrorCode::InvalidPackage => "The generated tool package is not supported.",
        CapabilityErrorCode::InvalidInput => "The generated tool input is invalid.",
        CapabilityErrorCode::IntegrityError => "The generated tool failed its integrity check.",
        CapabilityErrorCode::VerificationFailed | CapabilityErrorCode::NotValidated => {
            "The generated tool is not verified for the current runtime."
        }
        CapabilityErrorCode::NotActive | CapabilityErrorCode::StaleRevision => {
            "The generated tool is no longer active."
        }
        CapabilityErrorCode::Conflict => "The generated tool changed while it was called.",
        CapabilityErrorCode::Busy => "The generated tool runtime is busy. Try again.",
        CapabilityErrorCode::Timeout => "The generated tool timed out.",
        CapabilityErrorCode::Cancelled => "The generated tool call was cancelled.",
        CapabilityErrorCode::OutputLimit => "The generated tool produced too much output.",
        CapabilityErrorCode::ProtocolError => {
            "The generated tool runtime returned an invalid response."
        }
        CapabilityErrorCode::StorageError => "The generated tool result could not be recorded.",
    }
}
