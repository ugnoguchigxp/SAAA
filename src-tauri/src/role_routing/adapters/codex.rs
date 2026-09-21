//! Process boundary for the bundled, capability-restricted Codex sidecar.
//!
//! This module deliberately owns the child process rather than reusing the application's
//! app-server process. Role routing must never inherit the user's workspace, MCP servers, or
//! native tool permissions.
use super::codex_protocol::{FrameValidator, SidecarEvent};
use crate::{process_guard::ProcessGuard, RunCancellation};
use serde_json::{json, Value};
use std::{
    env,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, OnceLock},
    thread,
    time::{Duration, Instant},
};

const CANCEL_GRACE: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) static BUNDLED_ROLE_ROUTING_CODEX_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone)]
pub(crate) struct SidecarRequest {
    pub(crate) id: String,
    pub(crate) step_id: String,
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) output_schema: Option<Value>,
    pub(crate) timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidecarOutcome {
    Result(String),
    Failed(String),
    Cancelled,
}

pub(crate) fn bundled_sidecar_path() -> Result<PathBuf, String> {
    BUNDLED_ROLE_ROUTING_CODEX_PATH
        .get()
        .filter(|path| path.is_file())
        .cloned()
        .ok_or_else(|| "Role-routing Codex sidecar is unavailable".to_string())
}

pub(crate) fn run(
    request: &SidecarRequest,
    cancellation: &RunCancellation,
) -> Result<SidecarOutcome, String> {
    run_at(&bundled_sidecar_path()?, request, cancellation)
}

/// Runs one request over a fresh JSONL sidecar process. `executable` is explicit so test
/// fixtures can prove argv, EOF and cancellation behavior without a Codex credential.
pub(crate) fn run_at(
    executable: &Path,
    request: &SidecarRequest,
    cancellation: &RunCancellation,
) -> Result<SidecarOutcome, String> {
    if request.id.is_empty()
        || request.step_id.is_empty()
        || request.model.is_empty()
        || request.prompt.is_empty()
        || !(1_000..=300_000).contains(&request.timeout_ms)
    {
        return Err("Invalid role-routing sidecar request".into());
    }
    if cancellation.is_cancelled() {
        return Ok(SidecarOutcome::Cancelled);
    }

    let mut command = Command::new(executable);
    command.env_clear();
    configure_sidecar_environment(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = ProcessGuard::new(
        command
            .spawn()
            .map_err(|error| format!("Could not start role-routing Codex sidecar: {error}"))?,
    );
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "Role-routing Codex sidecar did not expose stdout".to_string())?;
    let (sender, receiver) = mpsc::sync_channel(16);
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    break;
                }
            }
        }
    });
    let stdin = child
        .child_mut()
        .stdin
        .as_mut()
        .ok_or_else(|| "Role-routing Codex sidecar did not expose stdin".to_string())?;
    let frame = json!({
        "version": 1,
        "id": request.id,
        "op": "run",
        "stepId": request.step_id,
        "model": request.model,
        "prompt": request.prompt,
        "outputSchema": request.output_schema,
        "timeoutMs": request.timeout_ms,
    });
    write_frame(stdin, &frame)?;

    let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
    let mut validator = FrameValidator::new(&request.id, &request.step_id);
    let mut cancel_sent_at = None;
    let outcome = loop {
        if cancellation.is_cancelled() && cancel_sent_at.is_none() {
            let cancel = json!({
                "version": 1,
                "id": request.id,
                "op": "cancel",
                "stepId": request.step_id,
            });
            write_frame(stdin, &cancel)?;
            cancel_sent_at = Some(Instant::now());
        }
        if let Some(sent_at) = cancel_sent_at {
            if sent_at.elapsed() >= CANCEL_GRACE {
                break Ok(SidecarOutcome::Cancelled);
            }
        } else if Instant::now() >= deadline {
            break Err("Role-routing Codex sidecar timed out".into());
        }
        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(Ok(line)) => match validator.validate(line.trim_ascii_end())? {
                SidecarEvent::Started | SidecarEvent::Activity => continue,
                SidecarEvent::Result { text } => break Ok(SidecarOutcome::Result(text)),
                SidecarEvent::Failed { code } => break Ok(SidecarOutcome::Failed(code)),
                SidecarEvent::Cancelled => break Ok(SidecarOutcome::Cancelled),
            },
            Ok(Err(error)) => break Err(format!("Could not read role-routing sidecar: {error}")),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(if validator.terminal() {
                    "Role-routing Codex sidecar closed after its terminal frame".into()
                } else {
                    "Role-routing Codex sidecar exited without a terminal frame".into()
                })
            }
        }
    };
    child.terminate();
    let _ = reader.join();
    outcome
}

/// Preserve only the runtime prerequisites used by the fixed sidecar. `HOME` lets the bundled
/// SDK read the user's existing Codex login, while the sidecar itself creates an empty cwd and
/// supplies an explicit no-MCP/no-network configuration. Provider keys, proxies and all other
/// parent process state are intentionally not inherited.
fn configure_sidecar_environment(command: &mut Command) {
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn write_frame(stdin: &mut impl Write, frame: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *stdin, frame)
        .map_err(|error| format!("Could not encode role-routing sidecar request: {error}"))?;
    stdin
        .write_all(b"\n")
        .and_then(|_| stdin.flush())
        .map_err(|error| format!("Could not write to role-routing Codex sidecar: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture(script: &str) -> (TempDir, PathBuf) {
        let directory = TempDir::new().expect("temporary fixture directory");
        let executable = directory.path().join("fake-sidecar");
        fs::write(&executable, format!("#!/bin/sh\n{script}\n")).expect("write fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&executable, permissions).expect("set executable");
        }
        (directory, executable)
    }

    fn request() -> SidecarRequest {
        SidecarRequest {
            id: "root".into(),
            step_id: "step".into(),
            model: "gpt-5.6-sol".into(),
            prompt: "hello".into(),
            output_schema: None,
            timeout_ms: 1_000,
        }
    }

    #[test]
    fn rr_20_process_requires_a_terminal_frame() {
        let (_directory, executable) = fixture("read line; printf '%s\\n' '{\"version\":1,\"id\":\"root\",\"stepId\":\"step\",\"op\":\"started\"}'");
        // This case tests EOF, not scheduler/process startup latency under the full suite.
        let mut input = request();
        input.timeout_ms = 10_000;
        let error = run_at(&executable, &input, &RunCancellation::default())
            .expect_err("missing terminal must fail");
        assert!(error.contains("without a terminal frame"), "{error}");
    }

    #[test]
    fn rr_20_process_accepts_only_valid_result_protocol() {
        let (_directory, executable) = fixture("read line; printf '%s\\n' '{\"version\":1,\"id\":\"root\",\"stepId\":\"step\",\"op\":\"started\"}' '{\"version\":1,\"id\":\"root\",\"stepId\":\"step\",\"op\":\"result\",\"text\":\"done\"}'");
        assert_eq!(
            run_at(&executable, &request(), &RunCancellation::default()).expect("valid result"),
            SidecarOutcome::Result("done".into())
        );
    }

    #[test]
    fn rr_20_cancel_sends_protocol_cancel_then_reaps_child() {
        let (_directory, executable) = fixture("read line; read line; printf '%s\\n' '{\"version\":1,\"id\":\"root\",\"stepId\":\"step\",\"op\":\"cancelled\"}'");
        let cancellation = RunCancellation::default();
        let trigger = cancellation.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            trigger.cancel();
        });
        assert_eq!(
            run_at(&executable, &request(), &cancellation).expect("cancel result"),
            SidecarOutcome::Cancelled
        );
        canceller.join().expect("canceller completes");
    }

    #[test]
    fn rr_20_config_isolation_clears_parent_environment_and_arguments() {
        let (_directory, executable) = fixture("test \"$#\" -eq 0 && test -n \"$PATH\" && test -z \"$OPENAI_API_KEY\" && test -z \"$HTTP_PROXY\" && test -z \"$HTTPS_PROXY\" || exit 9; read line; printf '%s\\n' '{\"version\":1,\"id\":\"root\",\"stepId\":\"step\",\"op\":\"result\",\"text\":\"isolated\"}'");
        assert_eq!(
            run_at(&executable, &request(), &RunCancellation::default())
                .expect("isolated fixture result"),
            SidecarOutcome::Result("isolated".into())
        );
    }
}
