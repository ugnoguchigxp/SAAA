//! Workspace-scoped read boundary for delegated profiles.
use std::path::{Path, PathBuf};

pub(crate) fn assert_read_path(workspace: &Path, requested: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(workspace).map_err(|_| "workspace_missing")?;
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let canonical = std::fs::canonicalize(&candidate).map_err(|_| "read_escape")?;
    if !canonical.starts_with(&root) {
        return Err("read_escape".into());
    }
    if is_trusted_state(&root, &canonical) {
        return Err("trusted_state_excluded".into());
    }
    Ok(canonical)
}

fn is_trusted_state(root: &Path, path: &Path) -> bool {
    path.starts_with(root.join(".saaa").join("delegated-sdk-state"))
        || path.starts_with(root.join(".codex"))
}

pub(crate) fn reject_write_or_network(operation: &str) -> Result<(), String> {
    if matches!(
        operation,
        "write" | "shell" | "network" | "test_run_unregistered"
    ) {
        return Err(format!("operation_unsupported:{operation}"));
    }
    Ok(())
}
