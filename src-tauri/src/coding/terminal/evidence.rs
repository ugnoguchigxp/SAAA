use super::verification_process::check;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};
pub(super) fn snapshot(workspace: &Path) -> Result<Value, String> {
    let status = git(workspace, &["status", "--porcelain=v1", "-z"])?;
    let head = check(
        workspace,
        &[
            "git".into(),
            "rev-parse".into(),
            "--verify".into(),
            "HEAD".into(),
        ],
        10,
        &|| false,
    )?;
    let diff = if head["exitCode"] == 0 {
        git(workspace, &["diff", "--binary", "HEAD", "--"])?
    } else {
        git(workspace, &["diff", "--binary", "--cached", "--"])?
    };
    let untracked = git(
        workspace,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    if untracked["truncated"] == true {
        return Err("terminal_untracked_evidence_budget_exhausted".into());
    }
    let mut files = Vec::new();
    let mut budget = 32 * 1024 * 1024u64;
    for file in untracked["output"]
        .as_str()
        .unwrap_or_default()
        .split('\0')
        .filter(|s| !s.is_empty())
    {
        if files.len() >= 1000 {
            return Err("terminal_untracked_evidence_budget_exhausted".into());
        }
        let path = workspace.join(file);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| "terminal_evidence_file_missing")?;
        let hash = if metadata.file_type().is_symlink() {
            format!(
                "{:x}",
                Sha256::digest(
                    std::fs::read_link(&path)
                        .map_err(|_| "terminal_evidence_symlink_invalid")?
                        .as_os_str()
                        .as_encoded_bytes()
                )
            )
        } else {
            if metadata.len() > budget {
                return Err("terminal_untracked_evidence_budget_exhausted".into());
            }
            budget -= metadata.len();
            let mut input =
                std::fs::File::open(path).map_err(|_| "terminal_evidence_file_missing")?;
            let mut hash = Sha256::new();
            let mut chunk = [0u8; 8192];
            let mut bytes = 0;
            loop {
                let n = input
                    .read(&mut chunk)
                    .map_err(|_| "terminal_evidence_read_failed")?;
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                if bytes > metadata.len() + 8192 {
                    return Err("terminal_evidence_file_changed".into());
                }
                hash.update(&chunk[..n]);
            }
            format!("{:x}", hash.finalize())
        };
        files.push(json!({"path":file,"sha256":hash,"bytes":metadata.len()}));
    }
    Ok(
        json!({"status":status["output"],"statusTruncated":status["truncated"],"diffSha256":diff["outputSha256"],"diffBytes":diff["outputBytes"],"head":if head["exitCode"]==0{head["output"].clone()}else{Value::Null},"untracked":files}),
    )
}
fn git(workspace: &Path, args: &[&str]) -> Result<Value, String> {
    let result = check(
        workspace,
        &std::iter::once("git".to_owned())
            .chain(args.iter().map(|s| s.to_string()))
            .collect::<Vec<_>>(),
        10,
        &|| false,
    )?;
    if result["exitCode"] != 0 || result["timeout"] == true || result["streamIncomplete"] == true {
        return Err("terminal_git_evidence_failed".into());
    }
    Ok(result)
}
