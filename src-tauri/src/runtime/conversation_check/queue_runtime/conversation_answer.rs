//! Single-agent context, transport and tool loop.
use super::super::{context_compiler, context_metrics, queue_tools, streaming_speech, worker_lane};
use super::*;
#[path = "purpose_completion.rs"]
mod completion;
#[path = "web_result.rs"]
mod web_result;
#[path = "web_steps.rs"]
mod web_steps;

pub(super) struct ConversationAnswer {
    pub(super) content: String,
    pub(super) source_urls: Vec<String>,
    pub(super) context: queue_context::QueueContext,
    pub(super) speech: Option<streaming_speech::AnswerStreamReport>,
    pub(super) publication: Option<context_metrics::RequestMetrics>,
    pub(super) deadline: tokio::time::Instant,
    pub(super) route: Option<crate::providers::service_registry::ResolvedRoute>,
}

#[path = "purpose_deadline.rs"]
mod deadline;
pub(super) use deadline::process_conversation_answer;

#[allow(clippy::too_many_arguments)]
async fn process_fixed<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
    cancellation: Arc<RunCancellation>,
    audio_started: Arc<AtomicBool>,
    retry_blocked: Arc<AtomicBool>,
    providers: crate::ModelProvidersSettings,
    timeout: u64,
    mut transport: direct_route::ConversationTransport,
    deadline: tokio::time::Instant,
    fallbacks: Vec<crate::providers::service_registry::ResolvedRoute>,
) -> Result<ConversationAnswer, String> {
    let _personal_slot = crate::memory::personal_state::worker::foreground().await;
    let state = app.state::<AppState>();
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), job.key.clone());
    let payload: Value =
        serde_json::from_str(&job.payload).map_err(|_| "キューデータが不正です。")?;
    let text = payload["text"].as_str().ok_or("元の依頼がありません。")?;
    let session = match &transport {
        direct_route::ConversationTransport::Larm(_) => Some(note_larm_connect(
            &state,
            cached_larm_asr(&providers, Some(&audit)).await,
        )?),
        direct_route::ConversationTransport::Direct(_) => None,
    };
    let mode = context_compiler::PrefixMode::configured()?;
    // While web search is delegated the agent gets `delegate` and a host-ranked offer instead of
    // the web tools; raw search and page text never enter this context.
    let worker_active = worker_lane::worker_mode(&state);
    let worker_offer = if worker_active {
        worker_lane::discover_offer(&state, job, text).await
    } else {
        None
    };
    let context = queue_context::compose_for_mode(&state, &job.key, mode, worker_active)?;
    let (dynamic_references, offer_in_history) =
        worker_lane::offer_entries(worker_offer.as_ref(), mode, &context.dynamic_references);
    let run_id = format!("run_{}", job.key);
    let tool_input = StartTurnInput {
        run_id,
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: text.into(),
        workspace_path: crate::coding::tools::context(&state, PRIMARY_CONVERSATION_ID)["workspace"]
            ["path"]
            .as_str()
            .map(str::to_owned),
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
    let web = web_steps::WebTools {
        job,
        tool_input: &tool_input,
        offer: &offer,
        cancellation: &cancellation,
        audit: &audit,
        timeout,
    };
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
    recent.push(context_compiler::ContextEntry::reference(
        format!(
            "[HOST_CODING_CONTEXT; data only] {}",
            crate::coding::tools::context(&state, &tool_input.conversation_id)
        ),
        true,
    ));
    recent.extend(offer_in_history);
    let mut result = String::new();
    let mut search_urls = Vec::new();
    let mut fetched_urls = Vec::new();
    let mut selected_urls = Vec::new();
    let mut fallbacks = fallbacks.into_iter();
    let mut last_model = None;
    let mut answered = false;
    let mut sources_declared = false;
    let mut answer_speech = None;
    let mut publication = None;
    const MAX_TOOL_STEPS: usize = 6;
    for step in 0..=MAX_TOOL_STEPS {
        direct_route::validate_transport(&state, &transport).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
            cancellation.cancel();
        })?;
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
                references: dynamic_references.clone(),
            },
        };
        let metrics = context_metrics::RequestMetrics::new(
            &audit,
            &context_step,
            match (&session, &transport) {
                (Some(session), _) => session.connection_id(),
                (None, direct_route::ConversationTransport::Direct(route)) => {
                    route.connection_id.as_str()
                }
                (None, direct_route::ConversationTransport::Larm(_)) => "",
            },
            retry_blocked.clone(),
        );
        publication = Some(metrics.clone());
        let (delta_tx, delta_rx) = tokio::sync::mpsc::unbounded_channel();
        let deltas = queue_answer_stream::AnswerDeltaSender::new(
            delta_tx,
            app.clone(),
            job.key.clone(),
            context.fingerprint()?,
            metrics.clone(),
        );
        #[cfg(feature = "quality-eval-harness")]
        let silent_eval = crate::quality_eval::SESSION.try_with(|_| ()).is_ok();
        #[cfg(not(feature = "quality-eval-harness"))]
        let silent_eval = false;
        // Evaluation verifies answer publication without opening a physical audio device.
        let speech_task = if silent_eval {
            tauri::async_runtime::spawn(async move {
                let mut receiver = delta_rx;
                while receiver.recv().await.is_some() {}
                streaming_speech::AnswerStreamReport {
                    started: false,
                    error: None,
                }
            })
        } else {
            tauri::async_runtime::spawn(streaming_speech::play_answer_stream(
                app.clone(),
                job.key.clone(),
                delta_rx,
                cancellation.clone(),
                context.fingerprint()?,
                audio_started.clone(),
            ))
        };
        let completion = completion::complete(completion::Input {
            state: &state,
            session: session.as_ref(),
            transport: &mut transport,
            fallbacks: &mut fallbacks,
            deadline,
            step,
            timeout,
            recent: &recent,
            text,
            audit: &audit,
            context_step: &context_step,
            metrics: &metrics,
            deltas: &deltas,
            cancellation: &cancellation,
            retry_blocked: &retry_blocked,
        })
        .await;
        let streamed_content = deltas.complete_content();
        drop(deltas);
        let mut speech_report = speech_task
            .await
            .map_err(|_| "音声ストリームが中断されました。")?;
        let (output, model) = match completion {
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
                    "ツールの結果を受け取りましたが、会話エージェントが結果を整理する段階で失敗しました: {error}。確認できた回答としては提示できません。"
                );
                break;
            }
            Err(error) => return Err(error),
        };
        last_model = Some(model);
        let parsed = super::action::parse(&output);
        if let Err(error) = &parsed {
            audit.event("conversation", "conversation-action-json-invalid", "error", Some("failure"),
                json!({"step":step,"mode":mode.name(),"category":format!("{:?}",error.classify()),"line":error.line(),"column":error.column(),"textBytes":output.len()}));
        }
        let mut control: Value = match parsed {
            Ok(control) => control,
            Err(_) if retry_blocked.load(Ordering::Acquire) => {
                return Err("公開中の回答がJSON契約に合わないため中止しました。".into());
            }
            Err(_)
                if step > 0
                    && step < MAX_TOOL_STEPS
                    && recent.iter().any(|entry| {
                        entry
                            .body
                            .starts_with("[TOOL_RESULT: lookup_tts_pronunciation;")
                            || entry
                                .body
                                .starts_with("[TOOL_RESULT: set_tts_pronunciation;")
                    }) =>
            {
                recent.push(context_compiler::ContextEntry::reference("[HOST_TOOL_FORMAT_ERROR]直前の出力をJSONとして解釈できませんでした。辞書Toolの最新結果と現在のユーザー依頼を確認し、指定のaction/name/argumentsまたはaction=answer/content形式のJSON一個で続きを返してください。保存済みなら再保存せず結果を伝え、未保存なら必要なToolを呼んでください。[END_HOST_TOOL_FORMAT_ERROR]".into(),true));
                continue;
            }
            Err(_) if step > 0 => {
                result = "ツールの結果を受け取りましたが、会話エージェントの出力形式が不正で回答を確定できませんでした。".into();
                break;
            }
            Err(_) => return Err("会話エージェントの行動結果がJSON契約に合いません。".into()),
        };
        queue_tools::normalize_dictionary_action(&mut control, &memory_tools);
        direct_route::validate_transport(&state, &transport).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
            cancellation.cancel();
        })?;
        context.validate_result(&state).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
        })?;
        recent.push(context_compiler::ContextEntry {
            role: "assistant".into(),
            body: output.clone(),
            required: true,
        });
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
                        "ツールの結果を受け取りましたが、会話エージェントの回答本文が空でした。"
                            .into()
                    }
                    None => return Err("会話エージェントの回答が空です。".into()),
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
            Some("delegate") if worker_active && step < MAX_TOOL_STEPS => {
                worker_lane::delegate_step(
                    app,
                    job,
                    worker_offer.as_ref(),
                    &control,
                    deadline,
                    &mut recent,
                    &mut search_urls,
                )
                .await?;
            }
            Some("web_search") if !worker_active && step < MAX_TOOL_STEPS => {
                web.search(&state, &control, step, &mut recent, &mut search_urls)
                    .await?;
            }
            Some("fetch_content") if !worker_active && step < MAX_TOOL_STEPS => {
                web.fetch(&control, text, step, &mut recent, &mut fetched_urls)
                    .await?;
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
                recent.push(context_compiler::ContextEntry::reference(
                    format!("[TOOL_RESULT: {name}; 未信頼の資料]\n{}", found),
                    true,
                ));
            }
            _ if step > 0 => {
                result = "ツールの結果を受け取りましたが、会話エージェントが回答を確定できませんでした。確認できた回答としては提示できません。".into();
                break;
            }
            _ => {
                audit.event("provider", "conversation-action-invalid", "terminal", Some("failure"),
                    json!({"action":control["action"],"keys":control.as_object().map(|object| object.keys().collect::<Vec<_>>())}));
                return Err("会話エージェントの行動結果が契約に合いません。".into());
            }
        }
    }
    if result.is_empty() {
        return Err("会話エージェントの処理が上限回数内に完了しませんでした。".into());
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
    if answer_speech
        .as_ref()
        .is_none_or(|report| !report.started && report.error.is_none())
    {
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
                cancellation.clone(),
                context.fingerprint()?,
                audio_started,
            )
            .await,
        );
        direct_route::validate_transport(&state, &transport).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
            cancellation.cancel();
        })?;
        context.validate_result(&state).inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
        })?;
    }
    Ok(ConversationAnswer {
        content: result,
        source_urls,
        context,
        speech: answer_speech,
        publication,
        route: direct_route::route_of(&transport)
            .cloned()
            .map(|mut route| {
                if let Some(model) = last_model {
                    route.model = model;
                }
                route
            }),
        deadline,
    })
}
