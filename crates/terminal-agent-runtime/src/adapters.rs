use crate::{write_private, Spec};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn resolve_executable(configured: &str, cli: &str) -> Result<PathBuf, String> {
    if !matches!(cli, "codex" | "claude") {
        return Err("terminal_cli_required".into());
    }
    let candidate = if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if !p.is_absolute() {
            return Err("terminal_executable_must_be_absolute".into());
        }
        p
    } else {
        executable_directories()
            .into_iter()
            .map(|p| p.join(cli))
            .find(|p| p.is_file())
            .ok_or("terminal_cli_missing")?
    };
    let canonical = std::fs::canonicalize(&candidate).map_err(|_| "terminal_cli_missing")?;
    if !canonical.is_file() {
        return Err("terminal_cli_missing".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&canonical)
            .map_err(|_| "terminal_cli_missing")?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err("terminal_cli_not_executable".into());
        }
    }
    // Keep the bin symlink: a Node CLI may rely on the adjacent Node executable.
    if candidate.is_absolute() {
        Ok(candidate)
    } else {
        Ok(std::env::current_dir()
            .map_err(|_| "terminal_cli_missing")?
            .join(candidate))
    }
}
fn executable_directories() -> Vec<PathBuf> {
    let mut directories = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        directories.extend([
            home.join(".local/bin"),
            home.join(".local/share/mise/shims"),
            home.join(".cargo/bin"),
        ]);
    }
    directories
}
pub fn cli_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    let mut paths = executable_directories();
    if executable.is_absolute() {
        if let Some(directory) = executable.parent() {
            paths.insert(0, directory.to_path_buf());
        }
    }
    if let Ok(path) = std::env::join_paths(paths) {
        command.env("PATH", path);
    }
    command
}
pub fn configuration(directory: &Path, spec: &Spec) -> Result<(), String> {
    let arguments = [
        "--saaa-terminal-agent",
        "mcp",
        directory.to_str().ok_or("terminal_path_invalid")?,
    ];
    write_private(
        &directory.join("mcp.json"),
        &serde_json::to_vec(
            &json!({"mcpServers":{"saaa":{"command":spec.helper,"args":arguments}}}),
        )
        .unwrap(),
    )?;
    // Print-mode defer is a native Claude protocol; hooks never wait for a person.
    let hook = format!(
        "{} --saaa-terminal-agent hook {}",
        quote(&spec.helper)?,
        quote(directory)?
    );
    write_private(&directory.join("claude-settings.json"), &serde_json::to_vec(&json!({"hooks":{
        "PreToolUse":[{"matcher":"AskUserQuestion|Bash|Edit|Write","hooks":[{"type":"command","command":hook,"timeout":10}]}],
        "PostToolUseFailure":[{"hooks":[{"type":"command","command":hook,"timeout":10}]}],
        "Stop":[{"hooks":[{"type":"command","command":hook,"timeout":10}]}],
        "StopFailure":[{"hooks":[{"type":"command","command":hook,"timeout":10}]}]
    }})).unwrap())
}
fn quote(path: &Path) -> Result<String, String> {
    Ok(format!(
        "'{}'",
        path.to_str()
            .ok_or("terminal_path_invalid")?
            .replace('\'', "'\\''")
    ))
}
pub fn command(directory: &Path, spec: &Spec) -> Result<Command, String> {
    let mut command = cli_command(&spec.executable);
    command.current_dir(&spec.workspace);
    if spec.cli == "claude" {
        command
            .args([
                "-p",
                "--permission-mode",
                "default",
                "--allowedTools",
                "mcp__saaa__saaa_consult,mcp__saaa__saaa_finish",
                "--output-format",
                "stream-json",
                "--verbose",
                "--strict-mcp-config",
                "--permission-prompt-tool",
                "mcp__saaa__saaa_permission",
            ])
            .arg("--mcp-config")
            .arg(directory.join("mcp.json"))
            .arg("--settings")
            .arg(directory.join("claude-settings.json"));
        if let Some(session) = &spec.resume {
            command.arg("--resume").arg(session);
        }
    } else {
        let helper = serde_json::to_string(&spec.helper).map_err(|_| "terminal_path_invalid")?;
        let arguments = serde_json::to_string(&[
            "--saaa-terminal-agent",
            "mcp",
            directory.to_str().ok_or("terminal_path_invalid")?,
        ])
        .unwrap();
        command.args([
            "-c",
            &format!("mcp_servers.saaa.command={helper}"),
            "-c",
            &format!("mcp_servers.saaa.args={arguments}"),
            "-c",
            "mcp_servers.saaa.enabled_tools=[\"saaa_consult\",\"saaa_finish\"]",
            "-c",
            "mcp_servers.saaa.tools.saaa_consult.approval_mode=\"auto\"",
            "-c",
            "mcp_servers.saaa.tools.saaa_finish.approval_mode=\"auto\"",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
            "exec",
        ]);
        if let Some(session) = &spec.resume {
            command.arg("resume").arg(session);
        }
        command.arg("--json");
        // Resume inherits the same explicit workspace-write sandbox, without bypass flags.
    }
    if !spec.model.is_empty() {
        command.arg("--model").arg(&spec.model);
    }
    if spec.cli == "codex" {
        command.arg("-");
    }
    Ok(command)
}
