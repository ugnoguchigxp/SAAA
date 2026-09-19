mod contracts;
mod kit;

use crate::generated_capabilities::host::process::{
    self, Cancellation, RuntimeCommand, TransportError, TransportErrorKind,
};
use contracts::{parse_response, HostRequest, HostResponse, Operation, MAX_REQUEST_BYTES};
use kit::{TrustedKit, ValidatedKit};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Debug)]
struct HostTimeouts {
    inspect: Duration,
    verify: Duration,
    invoke: Duration,
}

impl Default for HostTimeouts {
    fn default() -> Self {
        Self {
            inspect: Duration::from_secs(15),
            verify: Duration::from_secs(30),
            invoke: Duration::from_secs(15),
        }
    }
}

#[derive(Clone, Debug)]
struct WasmHost {
    bun_path: PathBuf,
    trusted_kit: TrustedKit,
    timeouts: HostTimeouts,
}

#[derive(Debug)]
struct ExecutionRecord {
    response: HostResponse,
    stderr: String,
    llang_commit: String,
    bun_version: String,
}

#[derive(Debug)]
enum HostFailure {
    Kit(String),
    Request(String),
    Transport(TransportError),
    Response(String),
}

impl WasmHost {
    fn new(bun_path: PathBuf, trusted_kit: TrustedKit) -> Result<Self, HostFailure> {
        if !bun_path.is_absolute() || !bun_path.is_file() {
            return Err(HostFailure::Kit(
                "configured Bun path must be an absolute file path".into(),
            ));
        }
        Ok(Self {
            bun_path,
            trusted_kit,
            timeouts: HostTimeouts::default(),
        })
    }

    async fn execute(
        &self,
        request: &HostRequest,
        cancellation: &Cancellation,
    ) -> Result<ExecutionRecord, HostFailure> {
        self.execute_with_command(request, cancellation, None).await
    }

    async fn execute_with_command(
        &self,
        request: &HostRequest,
        cancellation: &Cancellation,
        command_override: Option<RuntimeCommand>,
    ) -> Result<ExecutionRecord, HostFailure> {
        let kit = kit::validate(&self.trusted_kit).map_err(HostFailure::Kit)?;
        let request_value = request.to_value().map_err(HostFailure::Request)?;
        validate_schema(
            &kit.request_schema().map_err(HostFailure::Kit)?,
            &request_value,
        )
        .map_err(HostFailure::Request)?;
        let request_bytes = serde_json::to_vec(&request_value)
            .map_err(|error| HostFailure::Request(error.to_string()))?;
        if request_bytes.len() > MAX_REQUEST_BYTES {
            return Err(HostFailure::Request("request exceeds 64 KiB".into()));
        }
        let command = command_override.unwrap_or_else(|| self.runtime_command(&kit));
        let timeout = match request.operation {
            Operation::Inspect => self.timeouts.inspect,
            Operation::Verify => self.timeouts.verify,
            Operation::Invoke => self.timeouts.invoke,
        };
        let output = process::execute(&command, &request_bytes, timeout, cancellation)
            .await
            .map_err(HostFailure::Transport)?;
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
            HostFailure::Transport(TransportError {
                kind: TransportErrorKind::Protocol,
                message: format!("runtime stdout is not exactly one JSON value: {error}"),
                exit_code: Some(0),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                child_pid: None,
                reaped: true,
            })
        })?;
        validate_schema(&kit.response_schema().map_err(HostFailure::Kit)?, &value)
            .map_err(HostFailure::Response)?;
        let response = parse_response(value, request).map_err(HostFailure::Response)?;
        Ok(ExecutionRecord {
            response,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            llang_commit: kit.provenance_commit,
            bun_version: kit.bun_version,
        })
    }

    fn runtime_command(&self, kit: &ValidatedKit) -> RuntimeCommand {
        RuntimeCommand {
            executable: self.bun_path.clone(),
            arguments: vec![kit.runtime_path(), kit.candidate_path()],
            current_dir: kit.root.clone(),
        }
    }
}

fn validate_schema(schema: &Value, instance: &Value) -> Result<(), String> {
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

#[cfg(test)]
mod tests;
