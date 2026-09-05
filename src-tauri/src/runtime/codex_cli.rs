use serde_json::{json, Value};
use std::{
    env,
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::OnceLock,
};

pub(crate) static BUNDLED_CODEX_PATH: OnceLock<PathBuf> = OnceLock::new();
pub(crate) const MAX_CODEX_STDOUT_BYTES: u64 = 4 * 1_024 * 1_024;

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
