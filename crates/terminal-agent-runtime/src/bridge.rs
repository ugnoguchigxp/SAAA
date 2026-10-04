use crate::{append, Spec};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
};

fn question(
    directory: &Path,
    spec: &Spec,
    kind: &str,
    input: &Value,
    id: Option<&str>,
) -> Result<(), String> {
    append(
        directory,
        spec,
        "question",
        json!({"questionId":id.map(str::to_owned).unwrap_or_else(||uuid::Uuid::new_v4().simple().to_string()),"kind":kind,"input":input}),
    )
}
pub fn hook(directory: &Path, spec: &Spec) -> Result<(), String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("hook_input_too_large".into());
    }
    let input: Value = serde_json::from_slice(&bytes).map_err(|_| "hook_input_invalid")?;
    let name = input["hook_event_name"]
        .as_str()
        .ok_or("hook_name_missing")?;
    if name == "PreToolUse" && input["tool_name"] == "AskUserQuestion" {
        let tool = &input["tool_input"];
        if let Some(answer) = &spec.answer {
            if answer["kind"] == "question_cancel" && answer["input"] == *tool {
                println!(
                    "{}",
                    json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"The user cancelled the saved question. Follow only their new explicit request."}})
                );
                return Ok(());
            }
            if answer["kind"] == "question" && answer["input"] == *tool {
                let mut updated = tool.clone();
                updated["answers"] = answer["answer"].clone();
                println!(
                    "{}",
                    json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":updated}})
                );
                return Ok(());
            }
        }
        question(
            directory,
            spec,
            "question",
            tool,
            input["tool_use_id"].as_str(),
        )?;
        println!(
            "{}",
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"defer","permissionDecisionReason":"SAAA saved the question. Resume this exact session after the host supplies the answer."}})
        );
    } else if name == "PreToolUse" {
        let tool_name = input["tool_name"].as_str().unwrap_or("");
        let tool_input = &input["tool_input"];
        let inside = tool_input["file_path"].as_str().is_some_and(|path| {
            let path = std::path::PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                spec.workspace.join(path)
            };
            let existing = if path.exists() {
                path.clone()
            } else {
                path.parent().unwrap_or(&path).to_path_buf()
            };
            std::fs::canonicalize(existing).is_ok_and(|p| p.starts_with(&spec.workspace))
                && !path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
        });
        let decision = if matches!(tool_name, "Edit" | "Write") && inside {
            "allow"
        } else {
            "ask"
        };
        println!(
            "{}",
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":decision,"permissionDecisionReason":"SAAA permits changes only inside the selected workspace. Commands and other paths need a saved human decision."}})
        );
    } else {
        append(directory, spec, "hook", input)?;
        println!("{{}}");
    }
    Ok(())
}
fn tools() -> Value {
    json!([
        {"name":"saaa_consult","annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false},"description":"Ask the SAAA host about a blocker or ambiguous requirement. Returns immediately; on awaiting_user stop this turn and await exact-session resume.","inputSchema":{"type":"object","additionalProperties":false,"properties":{"question":{"type":"string","maxLength":8000},"options":{"type":"array","items":{"type":"string"},"maxItems":12}},"required":["question"]}},
        {"name":"saaa_finish","annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false},"description":"Save a completion candidate after finishing the original requested work. The host runs saved acceptance commands. Never omit remaining human/manual checks. This does not itself declare task completion.","inputSchema":{"type":"object","additionalProperties":false,"properties":{"summary":{"type":"string","maxLength":8000},"remainingManualChecks":{"type":"array","maxItems":32,"items":{"type":"string","maxLength":1000}}},"required":["summary","remainingManualChecks"]}},
        {"name":"saaa_permission","description":"Host permission decision for Claude Code. Only an explicit saved human approval of the identical tool input permits execution.","inputSchema":{"type":"object","properties":{"tool_name":{"type":"string"},"input":{"type":"object"},"tool_use_id":{"type":"string"}},"required":["tool_name","input"]}}
    ])
}
pub fn serve(directory: &Path, spec: &Spec) -> Result<(), String> {
    let mut input = BufReader::new(std::io::stdin());
    loop {
        let mut bytes = Vec::new();
        let size = input
            .by_ref()
            .take(65537)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if size == 0 {
            return Ok(());
        }
        if size > 65536 {
            return Err("mcp_input_too_large".into());
        }
        let request: Value = serde_json::from_slice(&bytes).map_err(|_| "mcp_request_invalid")?;
        if request.get("id").is_none() {
            continue;
        }
        let outcome = match request["method"].as_str() {
            Some("initialize") => Ok(
                json!({"protocolVersion":request["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),"capabilities":{"tools":{}},"serverInfo":{"name":"saaa-terminal-host","version":"1.0.0"}}),
            ),
            Some("ping") => Ok(json!({})),
            Some("tools/list") => Ok(json!({"tools":tools()})),
            Some("tools/call") => call(directory, spec, &request["params"]),
            _ => Err("method_not_found".into()),
        };
        let response = match outcome {
            Ok(result) => json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
            Err(message) => {
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32602,"message":message}})
            }
        };
        println!("{response}");
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
}
fn call(directory: &Path, spec: &Spec, params: &Value) -> Result<Value, String> {
    let input = &params["arguments"];
    let result = match params["name"].as_str() {
        Some("saaa_consult") => {
            if input["question"]
                .as_str()
                .is_none_or(|v| v.trim().is_empty() || v.len() > 8000)
            {
                return Err("question_invalid".into());
            }
            question(directory, spec, "blocker", input, None)?;
            json!({"status":"awaiting_user","message":"Question persisted. Stop this turn; SAAA will resume this exact session with its decision."})
        }
        Some("saaa_finish") => {
            if input["summary"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty() || s.len() > 8000)
                || input["remainingManualChecks"].as_array().is_none_or(|a| {
                    a.len() > 32 || a.iter().any(|s| s.as_str().is_none_or(|s| s.len() > 1000))
                })
            {
                return Err("completion_candidate_invalid".into());
            }
            append(directory, spec, "candidate", input.clone())?;
            json!({"status":"candidate_saved","meaning":"The host must verify saved acceptance commands. This is not task completion."})
        }
        Some("saaa_permission") if spec.cli == "claude" => {
            if input["tool_name"].as_str().is_none() || !input["input"].is_object() {
                return Err("permission_invalid".into());
            }
            let mut canonical = input.clone();
            if let Some(object) = canonical.as_object_mut() {
                object.remove("tool_use_id");
            }
            if spec.answer.as_ref().is_some_and(|answer| {
                answer["kind"] == "permission"
                    && answer["input"] == canonical
                    && answer["answer"] == "approve"
            }) {
                let receipt = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join("permission-used"));
                if receipt.is_ok() {
                    json!({"behavior":"allow","updatedInput":input["input"]})
                } else {
                    json!({"behavior":"deny","message":"This one-time permission has already been consumed."})
                }
            } else {
                question(
                    directory,
                    spec,
                    "permission",
                    &canonical,
                    input["tool_use_id"].as_str(),
                )?;
                json!({"behavior":"deny","message":"SAAA saved this permission request. Stop and await the user's decision."})
            }
        }
        _ => return Err("tool_not_found".into()),
    };
    Ok(json!({"content":[{"type":"text","text":result.to_string()}],"isError":false}))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn spec(path: &Path) -> Spec {
        Spec {
            job: "j".into(),
            run: "r".into(),
            nonce: "n".repeat(32),
            workspace: path.into(),
            executable: "/bin/echo".into(),
            helper: "/bin/echo".into(),
            cli: "claude".into(),
            model: String::new(),
            prompt: String::new(),
            resume: None,
            answer: None,
            deadline_seconds: 10,
            wake_path: None,
        }
    }
    #[test]
    fn permission_is_denied_until_identical_human_decision_then_consumed_once() {
        let root = tempfile::tempdir().unwrap();
        let mut spec = spec(root.path());
        crate::create(root.path(), &spec).unwrap();
        let arguments =
            json!({"tool_name":"Bash","input":{"command":"bun test"},"tool_use_id":"id"});
        let request = json!({"name":"saaa_permission","arguments":arguments});
        let first = call(root.path(), &spec, &request).unwrap();
        assert!(first["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("deny"));
        spec.answer = Some(
            json!({"kind":"permission","input":{"tool_name":"Bash","input":{"command":"bun test"}},"answer":"approve"}),
        );
        assert!(
            call(root.path(), &spec, &request).unwrap()["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("allow")
        );
        assert!(
            call(root.path(), &spec, &request).unwrap()["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("deny")
        );
    }
    #[test]
    fn blocker_returns_immediately_after_durable_question() {
        let root = tempfile::tempdir().unwrap();
        let spec = spec(root.path());
        crate::create(root.path(), &spec).unwrap();
        let result=call(root.path(),&spec,&json!({"name":"saaa_consult","arguments":{"question":"Which color?","options":["Blue","Red"]}})).unwrap();
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("awaiting_user"));
        assert!(root.path().join("events.jsonl").is_file());
    }
}
