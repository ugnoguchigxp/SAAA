use serde_json::{json, Value};
use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{mpsc, OnceLock},
    thread,
    time::Duration,
};

use crate::process_guard::ProcessGuard;
use crate::redact::redact_runtime_text;
use crate::{CodexModelOption, CodexModelPage, CodexRuntimeStatus};

pub(crate) static BUNDLED_CODEX_PATH: OnceLock<PathBuf> = OnceLock::new();
pub(crate) const MAX_CODEX_STDOUT_BYTES: u64 = 4 * 1_024 * 1_024;

pub(crate) fn spawn_bounded_codex_reader<R>(
    stdout: R,
) -> (
    mpsc::Receiver<Result<Value, String>>,
    thread::JoinHandle<()>,
)
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(256);
    let reader = thread::spawn(move || {
        let mut stdout = BufReader::new(stdout.take(MAX_CODEX_STDOUT_BYTES + 1));
        let mut bytes_read = 0_u64;
        loop {
            let mut line = String::new();
            let count = match stdout.read_line(&mut line) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) => {
                    let _ = sender.send(Err(format!(
                        "Could not read Codex app-server response: {error}"
                    )));
                    break;
                }
            };
            bytes_read = bytes_read.saturating_add(count as u64);
            if bytes_read > MAX_CODEX_STDOUT_BYTES {
                let _ = sender.send(Err(
                    "Codex app-server output exceeded the 4 MiB limit".to_string()
                ));
                break;
            }
            let message = serde_json::from_str::<Value>(line.trim_end())
                .map_err(|error| format!("Codex app-server returned invalid JSON: {error}"));
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    (receiver, reader)
}

pub(crate) fn fetch_codex_status() -> Result<CodexRuntimeStatus, String> {
    let mut child = ProcessGuard::new(spawn_codex_app_server()?);
    let mut stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server stdin is unavailable".to_string())?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server stdout is unavailable".to_string())?;
    let (receiver, stdout_reader) = spawn_bounded_codex_reader(stdout);
    let result = (|| {
        write_codex_handshake(&mut stdin)?;
        write_codex_message(
            &mut stdin,
            json!({ "method": "account/read", "id": 2, "params": { "refreshToken": false } }),
        )?;
        let response = receive_codex_response(&receiver, 2, Duration::from_secs(15))?;
        let account = response.pointer("/result/account");
        let account_type = account
            .and_then(|account| account.get("type"))
            .and_then(Value::as_str)
            .filter(|value| value.chars().count() <= 80 && !value.chars().any(char::is_control))
            .map(str::to_string);
        let authenticated = account.is_some_and(|value| !value.is_null());
        Ok(CodexRuntimeStatus {
            installed: true,
            authenticated,
            runtime: "app-server".to_string(),
            account_type,
            message: if authenticated {
                "Codex is ready".to_string()
            } else {
                "Codex is installed but not authenticated. Run codex login.".to_string()
            },
        })
    })();
    drop(stdin);
    drop(receiver);
    child.terminate();
    if stdout_reader.join().is_err() && result.is_ok() {
        return Err("Codex output reader stopped unexpectedly".to_string());
    }
    result
}

pub(crate) fn fetch_codex_models() -> Result<Vec<CodexModelOption>, String> {
    let mut child = ProcessGuard::new(spawn_codex_app_server()?);
    let mut stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server stdin is unavailable".to_string())?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server stdout is unavailable".to_string())?;
    let (receiver, stdout_reader) = spawn_bounded_codex_reader(stdout);

    let result = (|| {
        write_codex_message(
            &mut stdin,
            json!({
                "method": "initialize",
                "id": 1,
                "params": {
                    "clientInfo": {
                        "name": "saaa",
                        "title": "SAAA",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
        )?;
        write_codex_message(&mut stdin, json!({ "method": "initialized", "params": {} }))?;

        let mut request_id = 2_u64;
        let mut cursor: Option<String> = None;
        let mut seen_cursors = std::collections::HashSet::new();
        let mut seen_model_ids = std::collections::HashSet::new();
        let mut models = Vec::new();
        let mut page_count = 0_usize;
        let lookup_deadline = std::time::Instant::now() + Duration::from_secs(60);

        loop {
            if page_count >= 20 {
                return Err("Codex model pagination exceeded the 20-page limit".to_string());
            }
            page_count += 1;
            let mut params = json!({ "limit": 100, "includeHidden": false });
            if let Some(value) = &cursor {
                params["cursor"] = Value::String(value.clone());
            }
            write_codex_message(
                &mut stdin,
                json!({ "method": "model/list", "id": request_id, "params": params }),
            )?;

            let page_deadline =
                (std::time::Instant::now() + Duration::from_secs(20)).min(lookup_deadline);
            let page = loop {
                let remaining = page_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err("Timed out while loading models from Codex".to_string());
                }
                let message = receiver
                    .recv_timeout(remaining)
                    .map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => {
                            "Timed out while loading models from Codex".to_string()
                        }
                        mpsc::RecvTimeoutError::Disconnected => {
                            "Codex app-server stopped before returning models".to_string()
                        }
                    })??;
                if message.get("id").and_then(Value::as_u64) != Some(request_id) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    let detail = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown Codex app-server error");
                    return Err(format!(
                        "Could not load Codex models: {}",
                        redact_runtime_text(detail)
                    ));
                }
                let result = message
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "Codex model response did not include a result".to_string())?;
                let page = serde_json::from_value::<CodexModelPage>(result)
                    .map_err(|error| format!("Could not decode Codex models: {error}"))?;
                for model in &page.data {
                    validate_codex_model_option(model)?;
                    if !seen_model_ids.insert(model.id.clone()) {
                        return Err("Codex model list contained a duplicate id".to_string());
                    }
                }
                break page;
            };

            if models.len().saturating_add(page.data.len()) > 2_000 {
                return Err("Codex model list exceeded the 2,000-item limit".to_string());
            }
            models.extend(page.data.into_iter().filter(|model| !model.hidden));
            cursor = match page.next_cursor {
                Some(next) if next.is_empty() || next.len() > 1_024 => {
                    return Err("Codex model cursor is invalid".to_string())
                }
                Some(next) if !seen_cursors.insert(next.clone()) => {
                    return Err("Codex model pagination repeated a cursor".to_string())
                }
                next => next,
            };
            if cursor.is_none() {
                break;
            }
            request_id = request_id
                .checked_add(1)
                .ok_or_else(|| "Codex model request id overflowed".to_string())?;
        }

        Ok(models)
    })();

    drop(stdin);
    drop(receiver);
    child.terminate();
    if stdout_reader.join().is_err() && result.is_ok() {
        return Err("Codex output reader stopped unexpectedly".to_string());
    }
    result
}

pub(crate) fn validate_codex_model_option(model: &CodexModelOption) -> Result<(), String> {
    fn valid(value: &str, max_chars: usize, allow_empty: bool) -> bool {
        (allow_empty || !value.is_empty())
            && value.chars().count() <= max_chars
            && !value.chars().any(char::is_control)
    }

    if !valid(&model.id, 160, false)
        || !valid(&model.model, 160, false)
        || !valid(&model.display_name, 200, true)
        || !valid(&model.description, 2_000, true)
        || model
            .default_reasoning_effort
            .as_deref()
            .is_some_and(|value| !valid(value, 80, false))
        || model.supported_reasoning_efforts.len() > 16
        || model.supported_reasoning_efforts.iter().any(|effort| {
            !valid(&effort.reasoning_effort, 80, false) || !valid(&effort.description, 500, true)
        })
        || model.input_modalities.len() > 16
        || model
            .input_modalities
            .iter()
            .any(|modality| !valid(modality, 80, false))
    {
        return Err("Codex model response contained invalid bounded fields".to_string());
    }
    Ok(())
}

pub(crate) fn write_codex_message(stdin: &mut impl Write, message: Value) -> Result<(), String> {
    writeln!(stdin, "{message}")
        .and_then(|_| stdin.flush())
        .map_err(|error| format!("Could not write to Codex app-server: {error}"))
}

pub(crate) fn write_codex_handshake(stdin: &mut impl Write) -> Result<(), String> {
    write_codex_message(
        stdin,
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "saaa",
                    "title": "SAAA",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": null
            }
        }),
    )?;
    write_codex_message(stdin, json!({ "method": "initialized", "params": {} }))
}

pub(crate) fn receive_codex_response(
    receiver: &mpsc::Receiver<Result<Value, String>>,
    request_id: u64,
    timeout: Duration,
) -> Result<Value, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("Timed out waiting for Codex app-server".to_string());
        }
        let message = receiver
            .recv_timeout(remaining)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    "Timed out waiting for Codex app-server".to_string()
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    "Codex app-server stopped before responding".to_string()
                }
            })??;
        if message.get("id").and_then(Value::as_u64) != Some(request_id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Err(format!(
                "Codex app-server error: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            ));
        }
        return Ok(message);
    }
}

pub(crate) fn spawn_codex_app_server() -> Result<Child, String> {
    let mut errors = Vec::new();
    for executable in codex_executable_candidates() {
        if executable.is_absolute() && !executable.exists() {
            continue;
        }
        let mut command = Command::new(&executable);
        configure_codex_environment(&mut command);
        match command
            .args([
                "--config",
                "mcp_servers={}",
                "--config",
                "web_search=\"disabled\"",
                "--config",
                "sandbox_workspace_write.network_access=false",
                "app-server",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(error) => errors.push(format!("{}: {error}", executable.display())),
        }
    }
    Err(format!(
        "Could not start the Codex runtime. Install @openai/codex-sdk or set SAAA_CODEX_PATH. {}",
        errors.join("; ")
    ))
}

pub(crate) fn configure_codex_environment(command: &mut Command) {
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "CODEX_HOME",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
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
}

pub(crate) fn codex_executable_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("SAAA_CODEX_PATH").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = BUNDLED_CODEX_PATH.get() {
        candidates.push(path.clone());
    }

    if let Some((package, target, executable)) = codex_platform_package() {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("node_modules")
                .join("@openai")
                .join(package)
                .join("vendor")
                .join(target)
                .join("bin")
                .join(executable),
        );
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("node_modules")
            .join(".bin")
            .join(if cfg!(windows) { "codex.cmd" } else { "codex" }),
    );
    candidates.push(PathBuf::from("codex"));
    candidates
}

pub(crate) fn codex_platform_package() -> Option<(&'static str, &'static str, &'static str)> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Some(("codex-darwin-arm64", "aarch64-apple-darwin", "codex")),
        ("macos", "x86_64") => Some(("codex-darwin-x64", "x86_64-apple-darwin", "codex")),
        ("linux", "aarch64") => Some(("codex-linux-arm64", "aarch64-unknown-linux-musl", "codex")),
        ("linux", "x86_64") => Some(("codex-linux-x64", "x86_64-unknown-linux-musl", "codex")),
        ("windows", "aarch64") => {
            Some(("codex-win32-arm64", "aarch64-pc-windows-msvc", "codex.exe"))
        }
        ("windows", "x86_64") => Some(("codex-win32-x64", "x86_64-pc-windows-msvc", "codex.exe")),
        _ => None,
    }
}
