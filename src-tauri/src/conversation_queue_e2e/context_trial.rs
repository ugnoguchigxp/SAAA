//! Twenty-turn trial through the actual queue, transport, persisted answers and speech.
use super::*;
#[path = "context_trial/regressions.rs"]
mod regressions;

pub(super) fn respond(path: &str, body: &Value) -> Option<String> {
    if !path.starts_with("/llm/") {
        return None;
    }
    let messages = body["messages"].as_array()?;
    let text = messages.last()?["content"]
        .as_str()?
        .strip_prefix("Context試験:")?;
    let tools = messages
        .iter()
        .filter_map(|m| m["content"].as_str())
        .filter(|body| body.starts_with("[TOOL_RESULT:"))
        .count();
    if text.starts_with("03") && tools == 0 {
        return Some(json!({"action":"web_search","query":"fixture fact"}).to_string());
    }
    if text.starts_with("03") && tools == 1 {
        return Some(
            json!({"action":"fetch_content","url":"https://example.invalid/report"}).to_string(),
        );
    }
    if text.starts_with("partial-tool") && tools == 0 {
        return Some(json!({"action":"web_search","query":"partial fixture"}).to_string());
    }
    if text.starts_with("上限") {
        return Some(json!({"action":"web_search","query":"limit fixture"}).to_string());
    }
    if let Some(response) = regressions::response(text) {
        return Some(response);
    }
    Some(json!({"action":"answer","content":"Context基盤から確認します。Profileは次段階です。音声も短く確認します。","sources":[]}).to_string())
}

pub(super) fn transport_response(fixture: &Fixture, path: &str, body: &Value) -> Option<Response> {
    if !fixture.context_trial.load(Ordering::SeqCst) || !path.starts_with("/llm/") {
        return None;
    }
    let text = body["messages"].as_array()?.last()?["content"].as_str()?;
    if text.starts_with("Context失効:") {
        let writer = fixture.context_writer.lock().ok()?.clone()?;
        writer.write(|db| {
            if text.ends_with("source") {
                db.execute("DELETE FROM conversation_messages WHERE id='context-source'",[]).map_err(crate::database_error)?;
            } else if text.ends_with("rebind") {
                db.execute("UPDATE runtime_run_scopes SET relation='parent' WHERE run_id='run_context-scope-rebind' AND relation='current'",[]).map_err(crate::database_error)?;
            } else {
                db.execute("UPDATE context_scopes SET state='revoked' WHERE scope_key IN (SELECT scope_key FROM runtime_run_scopes WHERE run_id='run_context-scope-invalid' AND relation='current')",[]).map_err(crate::database_error)?;
            }
            Ok(())
        }).expect("fixture invalidation");
        let content =
            json!({"action":"answer","content":"失効根拠の回答は公開しない。","sources":[]})
                .to_string();
        let event = json!({"model":body["model"],"choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}]});
        return Some(
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                format!("data: {event}\n\ndata: [DONE]\n\n"),
            )
                .into_response(),
        );
    }
    if !text.starts_with("Context試験:") {
        return None;
    }
    if text.contains("fallback") && body["stream"] == true {
        return Some(Json(json!({"choices":[]})).into_response());
    }
    let partial_ready = !text.contains("partial-tool")
        || body["messages"].as_array()?.iter().any(|m| {
            m["content"]
                .as_str()
                .is_some_and(|s| s.starts_with("[TOOL_RESULT:"))
        });
    if text.contains("partial") && partial_ready {
        let event = json!({"model":body["model"],"choices":[{"index":0,"delta":{"content":"{\"action\":\"answer\",\"content\":\"途中の回答。"},"finish_reason":"length"}]});
        return Some(
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                format!("data: {event}\n\ndata: [DONE]\n\n"),
            )
                .into_response(),
        );
    }
    if text.contains("usage") {
        assert_eq!(body["stream_options"]["include_usage"], true);
        let content = respond(path, body)?;
        let usage = json!({"prompt_tokens":123,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens":21});
        let event = json!({"model":"fixture-concrete-model","choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}]});
        let usage_event = json!({"model":"fixture-concrete-model","choices":[],"usage":usage});
        return Some(
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                format!("data: {event}\n\ndata: {usage_event}\n\ndata: [DONE]\n\n"),
            )
                .into_response(),
        );
    }
    if text.contains("fallback") {
        assert!(body["stream_options"].is_null());
        return Some(Json(json!({"model":"fixture-concrete-model","usage":{"prompt_tokens":12,"completion_tokens":2},
            "choices":[{"message":{"content":respond(path,body)?},"finish_reason":"stop"}]})).into_response());
    }
    None
}

pub(super) async fn turn(
    state: &AppState,
    key: &str,
    text: &str,
    failure: bool,
) -> Result<(), String> {
    conversation_check::queue_runtime::enqueue_text(state, key.into(), text.into())?;
    state.conversation_queue_wake.notify_waiters();
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            let (failed, spoken, interrupted): (bool,bool,Option<String>) = state.sqlite_readers.read(|db| {
                let failed = db.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND state='failed')",[key],|r|r.get(0)).map_err(crate::database_error)?;
                let spoken = db.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND kind='speech' AND state='completed')",[key],|r|r.get(0)).map_err(crate::database_error)?;
                let interrupted = db.query_row("SELECT coalesce(error,'speech interrupted') FROM task_queue_jobs WHERE job_key=?1 AND kind='speech' AND state='interrupted' LIMIT 1",[key],|r|r.get(0)).optional().map_err(crate::database_error)?;
                Ok((failed,spoken,interrupted))
            })?;
            if let Some(error) = interrupted {
                return Err(format!("trial {key}: speech interrupted: {error}"));
            }
            if failed || spoken {
                if failed && !failure {
                    let error:Option<String>=state.sqlite_readers.read(|db|db.query_row("SELECT error FROM task_queue_jobs WHERE job_key=?1 AND state='failed' LIMIT 1",[key],|r|r.get(0)).optional().map_err(crate::database_error))?;
                    return Err(format!("trial {key}: {}", error.unwrap_or_else(|| "unknown failure".into())));
                }
                return if failed == failure { Ok(()) } else { Err(format!("trial {key} unexpected failed={failed}, spoken={spoken}")) };
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.map_err(|_| format!("trial {key} timed out"))?
}

pub(super) fn receipts(state: &AppState) -> Result<Vec<Value>, String> {
    state.sqlite_readers.read(|db| {
        let mut query = db.prepare("SELECT attributes_json FROM audit_events WHERE event_name IN ('conversation-context-attempt','conversation-context-request','conversation-context-visible') ORDER BY rowid").map_err(crate::database_error)?;
        let rows = query.query_map([], |r| r.get::<_,String>(0)).map_err(crate::database_error)?;
        rows.map(|r| serde_json::from_str(&r.map_err(crate::database_error)?).map_err(|e|e.to_string())).collect()
    })
}

pub(super) async fn verify(state: &AppState, fixture: &Fixture) -> Result<Value, String> {
    *fixture
        .context_writer
        .lock()
        .map_err(|_| "writer fixture lock")? = Some(state.sqlite_writer.clone());
    let before = fixture.requests.lock().map_err(|_| "request lock")?.len();
    for index in 1..=20 {
        let text = if index == 10 {
            format!("Context試験:{index:02} 初回は音声も短く確認する。Profileは含めない")
        } else {
            format!("Context試験:{index:02} SAAAのメモリ実装をContextから進める条件を確認する")
        };
        turn(state, &format!("context-turn-{index}"), &text, false).await?;
    }
    for (key, text, failure) in [
        ("context-usage", "Context試験:usage", false),
        ("context-fallback", "Context試験:fallback", false),
        ("context-partial", "Context試験:partial", true),
        ("context-partial-tool", "Context試験:partial-tool", true),
        ("context-limit", "Context試験:上限", false),
    ] {
        turn(state, key, text, failure).await?;
    }
    regressions::run(state).await?;
    let requests = fixture.requests.lock().map_err(|_| "request lock")?.clone();
    let requests = &requests[before..];
    let mode = std::env::var("SAAA_CONVERSATION_PREFIX_MODE").unwrap_or_else(|_| "legacy".into());
    let mut prefixes = std::collections::BTreeSet::new();
    for request in requests {
        let messages = request["messages"].as_array().ok_or("missing messages")?;
        if messages.iter().filter(|m| m["role"] == "system").count() != 1 {
            return Err("system count differs".into());
        }
        let current = messages.last().ok_or("missing input")?["content"]
            .as_str()
            .ok_or("missing current text")?;
        if messages.iter().filter(|m| m["content"] == current).count() != 1 {
            return Err("duplicated current input".into());
        }
        let fixed = messages[0]["content"]
            .as_str()
            .ok_or("missing fixed context")?;
        prefixes.insert(crate::providers::chat_completions::observation::digest(
            fixed,
        ));
        if mode == "stable" {
            if fixed.contains("[実行時の日時]") || fixed.contains("[TTS_DICTIONARY_PENDING;")
            {
                return Err("dynamic state in fixed context".into());
            }
            if messages
                .iter()
                .filter(|m| {
                    m["content"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("[HOST_RUNTIME_STATE]"))
                })
                .count()
                != 1
            {
                return Err("runtime count differs".into());
            }
            let runtime = messages
                .iter()
                .filter_map(|m| m["content"].as_str())
                .find(|body| body.starts_with("[HOST_RUNTIME_STATE]"))
                .ok_or("missing runtime")?;
            let state: Value =
                serde_json::from_str(runtime.lines().nth(1).ok_or("missing runtime JSON")?)
                    .map_err(|e| e.to_string())?;
            let tool_results = messages
                .iter()
                .filter_map(|m| m["content"].as_str())
                .filter(|body| body.starts_with("[TOOL_RESULT:"))
                .count();
            if state["remainingToolCalls"] != 6usize.saturating_sub(tool_results) {
                return Err("remaining tool count was stale".into());
            }
        }
    }
    if mode == "stable" && prefixes.len() != 1 {
        return Err(format!("stable prefix changed {} times", prefixes.len()));
    }
    let attempts: Vec<Value> = receipts(state)?
        .into_iter()
        .filter(|v| v["httpAttemptId"].is_string() && v["usageStatus"].is_string())
        .collect();
    if attempts.is_empty()
        || !attempts
            .iter()
            .any(|v| v["cacheReadTokens"] == 0 && v["inputTokens"] == 123)
        || !attempts
            .iter()
            .any(|v| v["requestMode"] == "json" && v["usageStatus"] == "provider")
    {
        return Err("transport usage observations missing".into());
    }
    let partial_attempts = requests
        .iter()
        .filter(|r| {
            r["messages"]
                .as_array()
                .and_then(|m| m.last())
                .is_some_and(|m| m["content"] == "Context試験:partial")
        })
        .count();
    if partial_attempts != 1 {
        return Err("partial output was retried".into());
    }
    let partial_tool_attempts = requests
        .iter()
        .filter(|r| {
            r["messages"]
                .as_array()
                .and_then(|m| m.last())
                .is_some_and(|m| m["content"] == "Context試験:partial-tool")
        })
        .count();
    if partial_tool_attempts != 2 {
        return Err("partial tool followup was retried".into());
    }
    if fixture
        .searches
        .lock()
        .map_err(|_| "search lock")?
        .iter()
        .filter(|query| query.as_str() == "limit fixture")
        .count()
        != 6
    {
        return Err("host tool limit was not enforced".into());
    }
    state.sqlite_readers.read(|db| {
        let saved:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id='reply_context-partial-tool')",[],|r|r.get(0)).map_err(crate::database_error)?;
        if saved {return Err("partial tool followup was replaced by another answer".into());}
        Ok(())
    })?;
    state.sqlite_writer.write(|db| { db.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('context-source',?1,'assistant','試験用の出典','9999999999999')",[PRIMARY_CONVERSATION_ID]).map_err(crate::database_error)?; Ok(()) })?;
    turn(state, "context-source-invalid", "Context失効:source", true).await?;
    turn(state, "context-scope-invalid", "Context失効:scope", true).await?;
    turn(state, "context-scope-rebind", "Context失効:rebind", true).await?;
    state.sqlite_readers.read(|db| {
        let count:i64=db.query_row("SELECT count(*) FROM conversation_messages WHERE id IN ('reply_context-source-invalid','reply_context-scope-invalid','reply_context-scope-rebind')",[],|r|r.get(0)).map_err(crate::database_error)?;
        if count!=0 {return Err("invalidated output was saved".into());}
        Ok(())
    })?;
    if fixture
        .spoken
        .lock()
        .map_err(|_| "speech lock")?
        .iter()
        .any(|s| s.contains("失効根拠の回答"))
    {
        return Err("invalidated output was spoken".into());
    }
    if receipts(state)?.iter().any(|row| {
        matches!(
            row["correlationId"].as_str(),
            Some("context-source-invalid" | "context-scope-invalid" | "context-scope-rebind")
        ) && row["firstVisibleMs"].is_number()
    }) {
        return Err("invalidated answer reached visible output".into());
    }
    regressions::verify(state, fixture)?;
    Ok(
        json!({"mode":mode,"turns":20,"requests":requests.len(),"fixedPrefixCount":prefixes.len(),
        "invalidationVerified":true,"usageVerified":true,"fallbackVerified":true,"partialNotRetried":true,"metrics":receipts(state)?}),
    )
}
