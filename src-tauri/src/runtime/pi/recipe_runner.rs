//! Executes a registered recipe with a fixed argv. The model cannot change the command.
use crate::steward::recipes;
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;
use std::process::Command;

pub(crate) fn run(
    connection: &Connection,
    recipe_id: &str,
    expected_digest: Option<&str>,
    workspace: &Path,
) -> Result<serde_json::Value, String> {
    let (argv, cwd, output_dir, timeout_ms) =
        recipes::load(connection, recipe_id, expected_digest)?;
    if argv.is_empty() {
        return Err("recipe_invalid".into());
    }
    let cwd_path = workspace.join(&cwd);
    let canonical_cwd = std::fs::canonicalize(&cwd_path).map_err(|_| "recipe_cwd_invalid")?;
    let root = std::fs::canonicalize(workspace).map_err(|_| "workspace_missing")?;
    if !canonical_cwd.starts_with(&root) {
        return Err("recipe_cwd_escape".into());
    }
    let output = workspace.join(&output_dir);
    if !output.starts_with(&root) && output_dir != "/tmp" && !output_dir.starts_with("/tmp/") {
        return Err("recipe_output_escape".into());
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).current_dir(&canonical_cwd);
    command.env_clear();
    command.env("PATH", std::env::var("PATH").unwrap_or_default());
    let _ = timeout_ms;
    let output = command.output().map_err(|_| "recipe_spawn_failed")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(json!({
        "exitCode": output.status.code().unwrap_or(1),
        "failures": if output.status.success() { json!([]) } else { json!([stderr.chars().take(500).collect::<String>()]) },
        "logDigest": format!("{:x}", md5ish(&stdout)),
        "cwd": canonical_cwd.to_string_lossy(),
    }))
}

fn md5ish(input: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}
