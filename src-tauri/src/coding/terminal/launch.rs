use crate::coding::contracts::CodingSettings;
use saaa_terminal_agent_runtime::{resolve_executable, Spec};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn validate(settings: &CodingSettings) -> bool {
    matches!(settings.terminal_kind.as_str(), "kitty" | "ghostty")
        && matches!(settings.terminal_cli.as_str(), "claude" | "codex")
        && settings.terminal_retry_limit <= 2
        && settings.terminal_executable.len() <= 4096
        && settings.terminal_model.len() <= 160
        && settings.terminal_checks.len() <= 8
        && settings.terminal_checks.iter().all(|args| {
            !args.is_empty()
                && args.len() <= 32
                && args
                    .iter()
                    .all(|arg| !arg.contains('\0') && arg.len() <= 4096)
                && !args[0].trim().is_empty()
        })
}
fn application(kind: &str) -> Result<PathBuf, String> {
    let name = match kind {
        "kitty" => "kitty",
        "ghostty" => "Ghostty",
        _ => return Err("terminal_default_required".into()),
    };
    let paths = [
        PathBuf::from("/Applications").join(format!("{name}.app")),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Applications")
            .join(format!("{name}.app")),
    ];
    paths
        .into_iter()
        .find(|p| p.is_dir())
        .ok_or("selected_terminal_missing".into())
}
pub fn probe(settings: &CodingSettings) -> Result<Value, String> {
    if !cfg!(target_os = "macos") {
        return Err("terminal_requires_macos".into());
    }
    if !validate(settings) {
        return Err("terminal_settings_invalid".into());
    }
    let app = application(&settings.terminal_kind)?;
    let executable = resolve_executable(&settings.terminal_executable, &settings.terminal_cli)?;
    let mut child = saaa_terminal_agent_runtime::cli_command(&executable)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "terminal_cli_unavailable")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child
        .try_wait()
        .map_err(|_| "terminal_probe_failed")?
        .is_none()
    {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("terminal_probe_timeout".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "terminal_probe_failed")?;
    if !output.status.success() || output.stdout.len() > 4096 {
        return Err("terminal_probe_failed".into());
    }
    Ok(
        json!({"available":true,"method":"terminal","cli":settings.terminal_cli,"terminal":settings.terminal_kind,"executable":executable,"application":app,"version":String::from_utf8_lossy(&output.stdout).trim(),"authentication":"existing CLI login; live request not tested"}),
    )
}
pub fn viewer(kind: &str, directory: &Path, helper: &Path) -> Result<(), String> {
    #[cfg(test)]
    if std::env::var_os("SAAA_TERMINAL_TEST_NO_VIEWER").is_some() {
        return Ok(());
    }
    let app = application(kind)?;
    let mut command = Command::new("/usr/bin/open");
    command.arg("-na").arg(app).arg("--args");
    if kind == "kitty" {
        command
            .arg("--title")
            .arg("SAAA · 作業状況")
            .arg("--directory")
            .arg(directory);
    } else {
        command.arg("--title=SAAA · 作業状況").arg("-e");
    }
    command
        .arg(helper)
        .args(["--saaa-terminal-agent", "view"])
        .arg(directory);
    let status = command.status().map_err(|_| "terminal_launch_failed")?;
    if !status.success() {
        return Err("terminal_launch_failed".into());
    }
    Ok(())
}
pub fn spec(
    settings: &CodingSettings,
    job: String,
    run: String,
    workspace: PathBuf,
    prompt: String,
    resume: Option<String>,
    answer: Option<Value>,
) -> Result<Spec, String> {
    Ok(Spec {
        job,
        run,
        nonce: uuid::Uuid::new_v4().simple().to_string(),
        workspace,
        executable: resolve_executable(&settings.terminal_executable, &settings.terminal_cli)?,
        helper: helper_executable()?,
        cli: settings.terminal_cli.clone(),
        model: settings.terminal_model.clone(),
        prompt,
        resume,
        answer,
        deadline_seconds: 1800,
        wake_path: super::host::wake_path(),
    })
}

fn helper_executable() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("SAAA_TERMINAL_TEST_HELPER") {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe().map_err(|_| "helper_executable_missing".into())
}
