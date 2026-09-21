use super::process::record;
use serde_json::{json, Value};
use std::{collections::HashSet, fs::File, io::BufReader, path::Path};
pub struct Session {
    pub id: String,
    pub leaf: Option<String>,
    pub summary: String,
    pub errors: usize,
    pub model_errors: usize,
    pub tools: Vec<Value>,
    pub complete: bool,
}
pub fn read(path: &Path, cwd: &Path, boundary: Option<&str>) -> Result<Session, String> {
    let file = File::open(path).map_err(|_| "session_missing")?;
    if file
        .metadata()
        .map_err(|_| "session_metadata_unavailable")?
        .len()
        > 64 * 1024 * 1024
    {
        return Err("session_output_limit".into());
    }
    let mut reader = BufReader::new(file);
    let header = record(&mut reader)?.ok_or("session_empty")?;
    let session_cwd = header["cwd"].as_str().ok_or("session_header_mismatch")?;
    if header["type"] != "session" || !same_workspace(session_cwd, cwd) {
        return Err("session_header_mismatch".into());
    }
    let id = header["id"]
        .as_str()
        .ok_or("session_id_missing")?
        .to_string();
    if id.len() > 160 {
        return Err("session_id_invalid".into());
    }
    let mut result = Session {
        id,
        leaf: None,
        summary: String::new(),
        errors: 0,
        model_errors: 0,
        tools: Vec::new(),
        complete: true,
    };
    let mut seen = HashSet::new();
    let mut collect = boundary.is_none();
    let mut bytes = 0;
    while let Some(entry) = record(&mut reader)? {
        bytes += entry.to_string().len();
        if bytes > 64 * 1024 * 1024 {
            return Err("session_output_limit".into());
        }
        let entry_id = entry["id"].as_str().ok_or("session_entry_id_missing")?;
        if entry_id.len() > 160 {
            return Err("session_entry_id_invalid".into());
        }
        if !seen.insert(entry_id.to_string()) {
            return Err("session_duplicate_entry".into());
        }
        if let Some(parent) = entry["parentId"].as_str() {
            if parent == entry_id || !seen.contains(parent) {
                return Err("session_parent_missing".into());
            }
        }
        result.leaf = Some(entry_id.into());
        if Some(entry_id) == boundary {
            collect = true;
            continue;
        }
        if !collect {
            continue;
        }
        if entry["type"] == "custom" && entry["customType"] == "saaa.codex-sdk.observation" {
            let data = &entry["data"];
            if data["isError"] == true {
                result.errors += 1;
            }
            if result.tools.len() < 30 {
                result.tools.push(json!({"entryId":entry_id,"tool":data["kind"].as_str().unwrap_or("unknown").chars().take(80).collect::<String>(),"isError":data["isError"]}));
            }
        }
        let m = &entry["message"];
        if m["stopReason"] == "error" {
            result.model_errors += 1;
        }
        if m["stopReason"] == "error" || m["stopReason"] == "aborted" || m["isError"] == true {
            result.errors += 1;
        }
        if m["role"] == "toolResult" && result.tools.len() < 30 {
            result
                .tools
                .push(json!({"entryId":entry_id,"tool":m["toolName"].as_str().unwrap_or("unknown").chars().take(80).collect::<String>(),"isError":m["isError"]}));
        }
        if m["role"] == "assistant" {
            if let Some(parts) = m["content"].as_array() {
                let text = parts
                    .iter()
                    .filter_map(|p| {
                        if p["type"] == "text" {
                            p["text"].as_str()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    result.complete &= text.chars().count() <= 2000;
                    result.summary = text.chars().take(2000).collect();
                }
            }
        }
    }
    if !collect {
        return Err("session_boundary_missing".into());
    }
    Ok(result)
}

fn same_workspace(session_cwd: &str, cwd: &Path) -> bool {
    if Path::new(session_cwd) == cwd {
        return true;
    }
    match (
        std::fs::canonicalize(session_cwd),
        std::fs::canonicalize(cwd),
    ) {
        (Ok(session_cwd), Ok(cwd)) => session_cwd == cwd,
        _ => false,
    }
}
