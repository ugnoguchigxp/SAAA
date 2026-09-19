use serde_json::Value;

use super::super::{contracts, errors::*, limits};

/// Serialises and size-checks a request, then validates it against the trusted runtime schema.
pub fn request_bytes(
    request: &contracts::HostRequest,
    request_schema: &Value,
) -> CapabilityResult<Vec<u8>> {
    let value = request
        .to_value()
        .map_err(|message| CapabilityError::new(CapabilityErrorCode::ProtocolError, message))?;
    validate_schema(request_schema, &value)
        .map_err(|message| CapabilityError::new(CapabilityErrorCode::ProtocolError, message))?;
    let bytes = serde_json::to_vec(&value).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::ProtocolError,
            "request is not serialisable",
        )
    })?;
    if bytes.len() > limits::MAX_REQUEST_BYTES {
        return error(CapabilityErrorCode::ProtocolError, "request exceeds 64 KiB");
    }
    Ok(bytes)
}

pub fn validate_schema(schema: &Value, instance: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("invalid JSON Schema: {error}"))?;
    if validator.is_valid(instance) {
        return Ok(());
    }
    let details = validator
        .iter_errors(instance)
        .take(3)
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!("JSON Schema validation failed: {details}"))
}

/// Stable mapping from a transport failure onto a capability error code.
pub fn transport_error(error: &super::process::TransportError) -> CapabilityError {
    use super::process::TransportErrorKind;
    let code = match error.kind {
        TransportErrorKind::Timeout => CapabilityErrorCode::Timeout,
        TransportErrorKind::Cancelled => CapabilityErrorCode::Cancelled,
        TransportErrorKind::StdoutLimit | TransportErrorKind::StderrLimit => {
            CapabilityErrorCode::OutputLimit
        }
        TransportErrorKind::Start => CapabilityErrorCode::Unavailable,
        _ => CapabilityErrorCode::ProtocolError,
    };
    CapabilityError::new(code, "host process failed")
}
