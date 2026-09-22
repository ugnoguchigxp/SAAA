//! Bun sidecar backend (migration/rollback only, WF-06 / WF-12).
//!
//! The current process implementation, isolated so the dispatcher can keep
//! it for rollback while the product path moves to webview. Removed for
//! macOS once the hidden E2E + search parity gates pass.

use std::{env, path::PathBuf, process::Stdio, sync::OnceLock, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

use super::super::agent_tools::{tool_error_content, AgentToolCall};

const MAX_SIDECAR_OUTPUT_BYTES: usize = 256 * 1024;

pub static BUNDLED_WEB_FETCH_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Execute one tool call through the sidecar process. Arguments must already
/// be the validated `{ "name", "arguments" }` envelope bytes.
pub async fn execute_envelope(request: &[u8], timeout: Duration) -> String {
    let Some(mut command) = sidecar_command() else {
        return tool_error_content(
            "web-fetch-unavailable",
            "The bundled WebFetch runtime is unavailable.",
        );
    };
    let mut child = match command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return tool_error_content(
                "web-fetch-unavailable",
                "The bundled WebFetch runtime could not be started.",
            );
        }
    };
    let Some(mut stdin) = child.stdin.take() else {
        return tool_error_content(
            "web-fetch-unavailable",
            "The bundled WebFetch runtime could not be started.",
        );
    };
    if stdin.write_all(request).await.is_err() || stdin.shutdown().await.is_err() {
        return tool_error_content(
            "web-fetch-unavailable",
            "The WebFetch request could not be sent.",
        );
    }
    drop(stdin);
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => project_sidecar_output(output.status.success(), &output.stdout),
        Ok(Err(_)) => tool_error_content(
            "web-fetch-unavailable",
            "The bundled WebFetch runtime stopped unexpectedly.",
        ),
        Err(_) => tool_error_content("TIMEOUT", "WebFetch exceeded the provider deadline."),
    }
}

pub fn envelope_for_call(call: &AgentToolCall) -> Result<Vec<u8>, String> {
    let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .map_err(|_| "Tool arguments do not match the WebFetch schema.".to_string())?;
    if !arguments.is_object() {
        return Err("Tool arguments do not match the WebFetch schema.".to_string());
    }
    serde_json::to_vec(&serde_json::json!({
        "name": call.name,
        "arguments": arguments
    }))
    .map_err(|_| "WebFetch is temporarily unavailable.".to_string())
}

fn sidecar_command() -> Option<Command> {
    let executable = env::var_os("SAAA_WEBFETCH_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            BUNDLED_WEB_FETCH_PATH
                .get()
                .filter(|path| path.is_file())
                .cloned()
        })
        .or_else(|| {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("bin")
                .join(if cfg!(windows) {
                    "webfetch.exe"
                } else {
                    "webfetch"
                });
            path.is_file().then_some(path)
        })?;
    let mut command = Command::new(executable);
    command.env_clear();
    for key in [
        "BRAVE_SEARCH_API_KEY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
    ] {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
    Some(command)
}

fn project_sidecar_output(success: bool, output: &[u8]) -> String {
    if !success || output.len() > MAX_SIDECAR_OUTPUT_BYTES {
        return tool_error_content(
            "web-fetch-unavailable",
            "The bundled WebFetch runtime returned an invalid response.",
        );
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(output) else {
        return tool_error_content(
            "web-fetch-unavailable",
            "The bundled WebFetch runtime returned an invalid response.",
        );
    };
    match value.get("ok").and_then(serde_json::Value::as_bool) {
        Some(true) => value.get("result").cloned().map_or_else(
            || {
                tool_error_content(
                    "web-fetch-unavailable",
                    "The bundled WebFetch runtime returned an invalid response.",
                )
            },
            |result| result.to_string(),
        ),
        Some(false) => value.get("error").cloned().map_or_else(
            || {
                tool_error_content(
                    "web-fetch-unavailable",
                    "The bundled WebFetch runtime returned an invalid response.",
                )
            },
            |error| serde_json::json!({ "error": error }).to_string(),
        ),
        None => tool_error_content(
            "web-fetch-unavailable",
            "The bundled WebFetch runtime returned an invalid response.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_envelope_is_projected_without_wrapper_or_unknown_details() {
        assert_eq!(
            project_sidecar_output(
                true,
                br#"{"ok":true,"result":{"type":"fetch_content_result","document":{"text":"safe"}}}"#,
            ),
            r#"{"document":{"text":"safe"},"type":"fetch_content_result"}"#
        );
        assert_eq!(
            project_sidecar_output(
                true,
                br#"{"ok":false,"error":{"code":"UNSAFE_URL","message":"blocked","retryable":false}}"#,
            ),
            r#"{"error":{"code":"UNSAFE_URL","message":"blocked","retryable":false}}"#
        );
        assert!(project_sidecar_output(true, b"not-json").contains("web-fetch-unavailable"));
    }
}
