pub mod process;
pub mod runtime_bundle;
pub mod wire;

use serde_json::Value;
use std::{path::Path, time::Duration};

use super::{
    contracts::{parse_response, HostRequest, HostResponse, Operation},
    errors::*,
    limits,
};
use runtime_bundle::TrustedRuntime;

#[derive(Clone, Debug)]
pub struct HostTimeouts {
    pub inspect: Duration,
    pub verify: Duration,
    pub invoke: Duration,
}

impl Default for HostTimeouts {
    fn default() -> Self {
        Self {
            inspect: limits::INSPECT_TIMEOUT,
            verify: limits::VERIFY_TIMEOUT,
            invoke: limits::INVOKE_TIMEOUT,
        }
    }
}

/// The trusted host. It always starts the fixed runtime entrypoint; only the candidate manifest
/// path varies between calls.
#[derive(Debug)]
pub struct WasmHost {
    bun_path: std::path::PathBuf,
    runtime: TrustedRuntime,
    pub timeouts: HostTimeouts,
}

impl WasmHost {
    pub fn new(runtime: TrustedRuntime, bun_path: std::path::PathBuf) -> CapabilityResult<Self> {
        if !bun_path.is_absolute() || !bun_path.is_file() {
            return error(
                CapabilityErrorCode::Unavailable,
                "configured Bun path is unavailable",
            );
        }
        Ok(Self {
            bun_path,
            runtime,
            timeouts: HostTimeouts::default(),
        })
    }

    pub fn runtime_digest(&self) -> &str {
        &self.runtime.digest
    }

    pub fn bun_version(&self) -> &str {
        &self.runtime.bun_version
    }

    pub fn llang_version(&self) -> &str {
        &self.runtime.llang_version
    }

    pub fn request_schema(&self) -> &Value {
        &self.runtime.request_schema
    }

    pub fn response_schema(&self) -> &Value {
        &self.runtime.response_schema
    }

    pub async fn execute(
        &self,
        request: &HostRequest,
        candidate_manifest: &Path,
        cancellation: &process::Cancellation,
    ) -> CapabilityResult<HostResponse> {
        self.execute_inner(request, candidate_manifest, cancellation, None)
            .await
    }

    /// Test-only override used to exercise hanging runtimes and malformed output. Production
    /// callers cannot select a command.
    #[cfg(test)]
    pub async fn execute_with_command(
        &self,
        request: &HostRequest,
        candidate_manifest: &Path,
        cancellation: &process::Cancellation,
        command: process::RuntimeCommand,
    ) -> CapabilityResult<HostResponse> {
        self.execute_inner(request, candidate_manifest, cancellation, Some(command))
            .await
    }

    async fn execute_inner(
        &self,
        request: &HostRequest,
        candidate_manifest: &Path,
        cancellation: &process::Cancellation,
        command_override: Option<process::RuntimeCommand>,
    ) -> CapabilityResult<HostResponse> {
        let request_bytes = wire::request_bytes(request, &self.runtime.request_schema)?;
        if !candidate_manifest.is_absolute() {
            return error(
                CapabilityErrorCode::IntegrityError,
                "candidate manifest path must be absolute",
            );
        }
        // The trusted bundle is re-checked immediately before every spawn, and the entrypoint
        // that was checked is the one the command runs.
        self.runtime.revalidate()?;
        let command = command_override.unwrap_or_else(|| self.runtime_command(candidate_manifest));
        let timeout = match request.operation {
            Operation::Inspect => self.timeouts.inspect,
            Operation::Verify => self.timeouts.verify,
            Operation::Invoke => self.timeouts.invoke,
        };
        let output = process::execute(&command, &request_bytes, timeout, cancellation)
            .await
            .map_err(|failure| wire::transport_error(&failure))?;
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::ProtocolError,
                "runtime stdout is not exactly one JSON value",
            )
        })?;
        wire::validate_schema(&self.runtime.response_schema, &value)
            .map_err(|message| CapabilityError::new(CapabilityErrorCode::ProtocolError, message))?;
        parse_response(value, request)
            .map_err(|message| CapabilityError::new(CapabilityErrorCode::ProtocolError, message))
    }

    fn runtime_command(&self, candidate_manifest: &Path) -> process::RuntimeCommand {
        process::RuntimeCommand {
            executable: self.bun_path.clone(),
            arguments: vec![
                self.runtime.entrypoint.clone(),
                candidate_manifest.to_path_buf(),
            ],
            current_dir: self.runtime.root.clone(),
        }
    }
}
