//! L-Lang backend adapter. It resolves the immutable revision recorded at registration time and
//! forwards to the existing `CapabilityService`; it never re-resolves a capability by name to a
//! newer revision.

#![allow(private_interfaces)]

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

use super::{BackendOutcome, BackendRequest, TechnicalStatus, ToolBackend};
use crate::generated_capabilities::contracts::{
    ContractField, FieldKind, InvokeRequest, ResolvedCapability, WasmContract,
};
use crate::generated_capabilities::errors::CapabilityErrorCode;
use crate::generated_capabilities::host::process::Cancellation;
use crate::generated_capabilities::service::CapabilityService;
use crate::RunCancellation;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LlangBinding {
    pub capability_id: String,
    pub revision_id: String,
    pub package_hash: String,
    pub contract_hash: String,
    pub catalog_epoch: i64,
    pub input_fields: Vec<String>,
}

impl LlangBinding {
    pub fn parse(binding: &Value) -> Option<Self> {
        serde_json::from_value(binding.clone()).ok()
    }

    fn resolved(&self) -> ResolvedCapability {
        ResolvedCapability {
            capability_id: self.capability_id.clone(),
            revision_id: self.revision_id.clone(),
            package_hash: self.package_hash.clone(),
            contract_hash: self.contract_hash.clone(),
            catalog_epoch: self.catalog_epoch,
            contract: WasmContract {
                version: 2,
                fields: self
                    .input_fields
                    .iter()
                    .map(|name| ContractField {
                        name: name.clone(),
                        kind: FieldKind::Boolean,
                        values: Vec::new(),
                        nullable: false,
                        undefinable: false,
                        optional: false,
                    })
                    .collect(),
            },
        }
    }
}

pub struct LlangBackend {
    pub(super) service: Option<Arc<CapabilityService>>,
}

impl LlangBackend {
    pub fn new(service: Option<Arc<CapabilityService>>) -> Self {
        Self { service }
    }
}

fn code_from_error(code: CapabilityErrorCode) -> &'static str {
    match code {
        CapabilityErrorCode::InvalidInput => "invalid-input",
        CapabilityErrorCode::NotActive | CapabilityErrorCode::StaleRevision => "stale-reference",
        CapabilityErrorCode::Busy | CapabilityErrorCode::Conflict => "capacity",
        CapabilityErrorCode::Cancelled => "cancelled",
        CapabilityErrorCode::Timeout => "timeout",
        CapabilityErrorCode::Disabled
        | CapabilityErrorCode::NotValidated
        | CapabilityErrorCode::Unavailable
        | CapabilityErrorCode::OutputLimit => "unavailable",
        _ => "integrity",
    }
}

fn boolean_inputs(binding: &LlangBinding, arguments: &Value) -> Result<Map<String, Value>, ()> {
    let object = arguments.as_object().ok_or(())?;
    let mut input = Map::new();
    for field in &binding.input_fields {
        let value = object.get(field).and_then(Value::as_bool).ok_or(())?;
        input.insert(field.clone(), Value::Bool(value));
    }
    if object.keys().any(|key| !binding.input_fields.contains(key)) {
        return Err(());
    }
    Ok(input)
}

#[async_trait]
impl ToolBackend for LlangBackend {
    async fn invoke(
        &self,
        request: BackendRequest,
        cancellation: &RunCancellation,
    ) -> BackendOutcome {
        let Some(service) = self.service.as_ref() else {
            return BackendOutcome::failed("unavailable");
        };
        let Some(binding) = LlangBinding::parse(&request.binding) else {
            return BackendOutcome::failed("integrity");
        };
        let input = match boolean_inputs(&binding, &request.arguments) {
            Ok(input) => input,
            Err(()) => return BackendOutcome::failed("invalid-input"),
        };
        let mut invocation = InvokeRequest::new(binding.resolved(), request.call_id.clone(), input);
        invocation.inner_timeout_ms =
            request.timeout.min(Duration::from_secs(30)).as_millis() as u64;
        invocation.origin = request.origin;
        invocation.actor = request.actor.clone();

        let inner = Cancellation::default();
        let future = service.invoke(invocation, &inner);
        tokio::pin!(future);
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                inner.cancel();
                Err(crate::generated_capabilities::errors::CapabilityError::new(
                    CapabilityErrorCode::Cancelled,
                    "cancelled",
                ))
            }
            result = &mut future => result,
        };
        match result {
            Ok(result) => BackendOutcome::succeeded(serde_json::json!({
                "ok": true,
                "callId": result.call_id,
                "revisionId": result.revision_id,
                "value": result.value,
            })),
            Err(error) => BackendOutcome {
                status: if error.code == CapabilityErrorCode::Cancelled {
                    TechnicalStatus::Cancelled
                } else {
                    TechnicalStatus::Failed
                },
                result: None,
                error_code: Some(code_from_error(error.code)),
            },
        }
    }
}
