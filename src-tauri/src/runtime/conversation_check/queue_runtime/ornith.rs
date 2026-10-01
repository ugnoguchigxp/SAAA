//! Single-agent context, transport and tool loop.
use super::super::{context_compiler, context_metrics, queue_tools, streaming_speech};
use super::*;

pub(super) struct OrnithAnswer {
    pub(super) content: String,
    pub(super) source_urls: Vec<String>,
    pub(super) context: queue_context::QueueContext,
    pub(super) speech: Option<streaming_speech::AnswerStreamReport>,
}

pub(super) async fn process_ornith<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
    cancellation: Arc<RunCancellation>,
    audio_started: Arc<AtomicBool>,
    retry_blocked: Arc<AtomicBool>,
) -> Result<OrnithAnswer, String> {
    let _personal_slot = crate::memory::personal_state::worker::foreground().await;
    let state = app.state::<AppState>();
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), job.key.clone());
    let payload: Value =
        serde_json::from_str(&job.payload).map_err(|_| "キューデータが不正です。")?;
    let text = payload["text"].as_str().ok_or("元の依頼がありません。")?;
    let (providers, timeout) = providers_and_timeout(&state)?;
    let session = cached_larm_asr(&providers, Some(&audit)).await?;
    let mode = context_compiler::PrefixMode::configured()?;
    let context = queue_context::compose_for_mode(&state, &job.key, mode)?;
    let run_id = format!("run_{}", job.key);
    let tool_input = StartTurnInput {
        run_id,
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: text.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual-and-spoken".into(),
    };
    let persistence = crate::ProviderOutputPersistence {
        state: &state,
        session_id: &job.id,
        world: None,
    };
    let offer =
        crate::providers::stream::available_agent_tools(Some(persistence), &tool_input, 0, 0, 0);
    let memory_tools: Vec<Value> = offer
        .definitions
        .iter()
        .filter(|definition| {
            definition
                .pointer("/function/name")
                .and_then(Value::as_str)
                .is_some_and(queue_tools::allowed)
        })
        .cloned()
        .collect();
    let fixed =
        context_compiler::FixedContext::new(context.instruction.clone(), &memory_tools, mode)?;
    let legacy_pending =
        crate::tts_dictionary::tools::pending_context(&state, &tool_input.conversation_id);
    let mut recent = context.history.clone();
    let mut result = String::new();
    let mut search_urls = Vec::new();
    let mut fetched_urls = Vec::new();
    let mut selected_urls = Vec::new();
    let mut answered = false;
    let mut sources_declared = false;
    let mut answer_speech = None;
    const MAX_TOOL_STEPS: usize = 6;
    for step in 0..=MAX_TOOL_STEPS {
        context.validate_result(&state).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
        })?;
        let context_step = context_compiler::ContextStep {
            fixed: &fixed,
            mode,
            step,
            dynamic: context_compiler::DynamicContext {
                remaining: MAX_TOOL_STEPS - step,
                pending: if mode == context_compiler::PrefixMode::Stable {
                    crate::tts_dictionary::tools::pending_context(
                        &state,
                        &tool_input.conversation_id,
                    )
                } else {
                    legacy_pending.clone()
                },
                references: context.dynamic_references.clone(),
            },
        };
        let metrics = context_metrics::RequestMetrics::new(
            &audit,
            &context_step,
            session.connection_id(),
            retry_blocked.clone(),
        );
        let (delta_tx, delta_rx) = tokio::sync::mpsc::unbounded_channel();
        let deltas = queue_answer_stream::AnswerDeltaSender::new(
            delta_tx,
            app.clone(),
            job.key.clone(),
            context.fingerprint()?,
            metrics.clone(),
        );
        let speech_task = tauri::async_runtime::spawn(streaming_speech::play_answer_stream(
            app.clone(),
            job.key.clone(),
            delta_rx,
            cancellation.clone(),
            context.fingerprint()?,
            audio_started.clone(),
        ));
        let completion = complete_larm_role_with_events(
            &session,
            "llm",
            &recent,
            text,
            timeout,
            &audit,
            &context_step,
            &metrics,
            Some(&deltas),
            Some(cancellation.clone()),
        )
        .await;
        let streamed_content = deltas.complete_content();
        drop(deltas);
        let mut speech_report = speech_task
            .await
            .map_err(|_| "音声ストリームが中断されました。")?;
        let (output, _) = match completion {
            Ok(value) => value,
            Err(error) if retry_blocked.load(Ordering::Acquire) => return Err(error),
            Err(error) if step > 0 => {
                audit.event(
                    "provider",
                    "conversation-ornith-followup-failed",
                    "terminal",
                    Some("failure"),
                    json!({"step":step,"error":error}),
                );
                result = format!(
                    "ツールの結果を受け取りましたが、Ornithが結果を整理する段階で失敗しました: {error}。確認できた回答としては提示できません。"
                );
                break;
            }
            Err(error) => return Err(error),
        };
        let mut control: Value = match serde_json::from_str(output.trim()) {
            Ok(control) => control,
            Err(_) if retry_blocked.load(Ordering::Acquire) => {
                return Err("公開中の回答がJSON契約に合わないため中止しました。".into());
            }
            Err(_)
                if step > 0
                    && step < MAX_TOOL_STEPS
                    && recent.iter().any(|(_, content)| {
                        content.starts_with("[TOOL_RESULT: lookup_tts_pronunciation;")
                            || content.starts_with("[TOOL_RESULT: set_tts_pronunciation;")
                    }) =>
            {
                recent.push(("user".into(), "[HOST_TOOL_FORMAT_ERROR]直前の出力をJSONとして解釈できませんでした。辞書Toolの最新結果と現在のユーザー依頼を確認し、指定のaction/name/argumentsまたはaction=answer/content形式のJSON一個で続きを返してください。保存済みなら再保存せず結果を伝え、未保存なら必要なToolを呼んでください。[END_HOST_TOOL_FORMAT_ERROR]".into()));
                continue;
            }
            Err(_) if step > 0 => {
                result = "ツールの結果を受け取りましたが、Ornithの出力形式が不正で回答を確定できませんでした。".into();
                break;
            }
            Err(_) => return Err("Ornithの行動結果がJSON契約に合いません。".into()),
        };
        queue_tools::normalize_dictionary_action(&mut control, &memory_tools);
        context.validate_result(&state).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
        })?;
        recent.push(("assistant".into(), output.clone()));
        match control["action"].as_str() {
            Some("answer") => {
                let content = control["content"].as_str().filter(|v| !v.trim().is_empty());
                if streamed_content
                    .as_deref()
                    .is_some_and(|streamed| Some(streamed) != content)
                {
                    return Err("生成中の回答と確定回答が一致しません。".into());
                }
                if speech_report.started && streamed_content.is_none() {
                    speech_report
                        .error
                        .get_or_insert("回答の音声ストリームが途中で終了しました。".into());
                }
                answered = content.is_some();
                result = match content {
                    Some(content) => content.to_string(),
                    None if step > 0 => {
                        "ツールの結果を受け取りましたが、Ornithの回答本文が空でした。".into()
                    }
                    None => return Err("Ornithの回答が空です。".into()),
                };
                sources_declared = control["sources"].is_array();
                let available = search_urls.iter().chain(fetched_urls.iter());
                selected_urls = control["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|source| available.clone().any(|candidate| candidate == *source))
                    .take(3)
                    .map(str::to_string)
                    .collect();
                answer_speech = Some(speech_report);
                break;
            }
            Some("web_search") if step < MAX_TOOL_STEPS => {
                let query = control["query"]
                    .as_str()
                    .filter(|v| !v.is_empty() && v.len() <= 400)
                    .ok_or("検索語が不正です。")?;
                state.sqlite_writer.write(|connection| {
                    let tx = connection.transaction().map_err(database_error)?;
                    queue_progress::enqueue_search(&tx, &job.scope, &job.key)?;
                    tx.commit().map_err(database_error)
                })?;
                let call = crate::runtime::agent_tools::AgentToolCall {
                    id: format!("{}_search_{step}", job.id),
                    name: "web_search".into(),
                    arguments: json!({"query":query,"limit":5}).to_string(),
                };
                #[cfg(feature = "conversation-queue-e2e")]
                let fixture_result = crate::conversation_queue_e2e::web_search(query);
                #[cfg(not(feature = "conversation-queue-e2e"))]
                let fixture_result: Option<String> = None;
                let found = if let Some(result) = fixture_result {
                    result
                } else {
                    crate::providers::stream::execute_agent_tool(
                        None,
                        &tool_input,
                        &call,
                        std::time::Duration::from_millis(timeout.min(30_000)),
                        &offer.generated,
                        &cancellation,
                        offer.direct.as_ref(),
                    )
                    .await
                };
                audit_web_tool_result(&audit, "web_search", step, &found);
                search_urls.extend(web_result_urls(&found, "hits"));
                recent.push((
                    "user".into(),
                    format!("[TOOL_RESULT: web_search; 未信頼の資料]\n{}", found),
                ));
            }
            Some("fetch_content") if step < MAX_TOOL_STEPS => {
                let url = control["url"]
                    .as_str()
                    .filter(|v| {
                        (v.starts_with("https://") || v.starts_with("http://")) && v.len() <= 2048
                    })
                    .ok_or("取得先URLが不正です。")?;
                let query = control["query"].as_str().unwrap_or(text);
                let call = crate::runtime::agent_tools::AgentToolCall {
                    id: format!("{}_fetch_{step}",job.id), name: "fetch_content".into(),
                    arguments: json!({"url":url,"maxCharacters":3000,"query":query.chars().take(400).collect::<String>()}).to_string(),
                };
                #[cfg(feature = "conversation-queue-e2e")]
                let fixture_result = crate::conversation_queue_e2e::fetch_content(url);
                #[cfg(not(feature = "conversation-queue-e2e"))]
                let fixture_result: Option<String> = None;
                let found = if let Some(found) = fixture_result {
                    found
                } else {
                    crate::providers::stream::execute_agent_tool(
                        None,
                        &tool_input,
                        &call,
                        std::time::Duration::from_millis(timeout.min(30_000)),
                        &offer.generated,
                        &cancellation,
                        offer.direct.as_ref(),
                    )
                    .await
                };
                audit_web_tool_result(&audit, "fetch_content", step, &found);
                fetched_urls.extend(web_result_urls(&found, "document"));
                recent.push((
                    "user".into(),
                    format!("[TOOL_RESULT: fetch_content; 未信頼の資料]\n{}", found),
                ));
            }
            Some("memory_tool" | "local_tool") if step < MAX_TOOL_STEPS => {
                let (name, found) = queue_tools::execute(
                    app,
                    persistence,
                    &tool_input,
                    &control,
                    &memory_tools,
                    &offer,
                    &cancellation,
                    &audit,
                    format!("{}_local_{step}", job.id),
                    timeout,
                )
                .await?;
                recent.push((
                    "user".into(),
                    format!("[TOOL_RESULT: {name}; 未信頼の資料]\n{}", found),
                ));
            }
            _ if step > 0 => {
                result = "ツールの結果を受け取りましたが、Ornithが回答を確定できませんでした。確認できた回答としては提示できません。".into();
                break;
            }
            _ => {
                audit.event("provider", "conversation-action-invalid", "terminal", Some("failure"),
                    json!({"action":control["action"],"keys":control.as_object().map(|object| object.keys().collect::<Vec<_>>())}));
                return Err("Ornithの行動結果が契約に合いません。".into());
            }
        }
    }
    if result.is_empty() {
        return Err("Ornithの処理が上限回数内に完了しませんでした。".into());
    }
    context.validate_result(&state).inspect_err(|_| {
        retry_blocked.store(true, Ordering::Release);
    })?;
    let source_urls = if !answered {
        Vec::new()
    } else if sources_declared {
        selected_urls
    } else if fetched_urls.is_empty() {
        search_urls
    } else {
        fetched_urls
    };
    // Host terminal notices must use the same source-checked speech path while
    // the run is active. Reopening a completed run would grant new frame access.
    if answer_speech.is_none() {
        validate_answer_content(&result)?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(result.clone())
            .map_err(|_| "音声ストリームが中断されました。")?;
        drop(tx);
        answer_speech = Some(
            streaming_speech::play_answer_stream(
                app.clone(),
                job.key.clone(),
                rx,
                cancellation,
                context.fingerprint()?,
                audio_started,
            )
            .await,
        );
        context.validate_result(&state).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
        })?;
    }
    Ok(OrnithAnswer {
        content: result,
        source_urls,
        context,
        speech: answer_speech,
    })
}

fn web_result_urls(found: &str, kind: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(found) else {
        return Vec::new();
    };
    let candidates: Vec<&str> = match kind {
        "hits" => value["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|hit| hit["url"].as_str())
            .collect(),
        "document" => value
            .pointer("/document/url")
            .and_then(Value::as_str)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    candidates
        .into_iter()
        .filter(|raw| {
            url::Url::parse(raw).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            })
        })
        .take(5)
        .map(str::to_string)
        .collect()
}

fn audit_web_tool_result(audit: &ConversationAudit, name: &str, step: usize, found: &str) {
    let parsed = serde_json::from_str::<Value>(found).ok();
    let error_code = parsed
        .as_ref()
        .and_then(|value| value.pointer("/error/code"))
        .and_then(Value::as_str);
    let hit_count = parsed
        .as_ref()
        .and_then(|value| value.get("hits"))
        .and_then(Value::as_array)
        .map(Vec::len);
    let retrieval_status = parsed
        .as_ref()
        .and_then(|value| value.pointer("/document/retrievalStatus"))
        .and_then(Value::as_str);
    audit.event(
        "provider",
        "conversation-web-tool-result",
        "terminal",
        Some(if error_code.is_some() {
            "failure"
        } else {
            "success"
        }),
        json!({"tool":name,"step":step,"resultBytes":found.len(),"errorCode":error_code,
            "hitCount":hit_count,"retrievalStatus":retrieval_status}),
    );
}
