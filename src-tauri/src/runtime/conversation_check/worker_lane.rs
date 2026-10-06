//! Worker lane glue: model transport, tool host, lane entry and the delegate round trip.
//!
//! The generic machinery lives in `worker_agents`; this module only binds it to the app: the
//! conversation direct route for model calls, the web tool host for tools, and the durable queue
//! lane. A worker never receives conversation history, memory or credentials: its model input is
//! the pinned profile context plus the delegated input.
use super::*;
use crate::task_queue::Job;
use crate::worker_agents::contracts::*;
use crate::worker_agents::executor::{self, Executor, JsonRunner, Runners};
use crate::worker_agents::{registry, web_search};
use async_trait::async_trait;
use std::time::Duration;
use tauri::{Manager, Runtime};

/// Margin kept for the conversation agent to answer after a synchronous wait.
const ANSWER_MARGIN: Duration = Duration::from_secs(5);

fn tier_route(transport: &direct_route::ConversationTransport) -> TierRoute {
    match direct_route::route_of(transport) {
        Some(route) if route.location == "cloud" => TierRoute {
            tier: Tier::Cloud,
            fingerprint: route.fingerprint.clone(),
            location: RouteLocation::Cloud,
        },
        Some(route) => TierRoute {
            tier: Tier::Local,
            fingerprint: route.fingerprint.clone(),
            location: RouteLocation::Local,
        },
        None => TierRoute {
            tier: Tier::Local,
            fingerprint: "larm".into(),
            location: RouteLocation::Local,
        },
    }
}

/// Model calls over the conversation route. v1 has one rung (the conversation route itself); a
/// cloud rung is only reachable when that route is a cloud route, and the executor then refuses
/// to call it without the approval flow.
struct AppWorkerModel<R: Runtime> {
    app: tauri::AppHandle<R>,
}

#[async_trait]
impl<R: Runtime> WorkerModel for AppWorkerModel<R> {
    fn tier_routes(&self) -> Result<Vec<TierRoute>, String> {
        let state = self.app.state::<AppState>();
        let (_providers, _legacy_timeout, transport, _fallbacks) =
            direct_route::prepare_transport(&state)?;
        Ok(vec![tier_route(&transport)])
    }

    async fn complete(
        &self,
        route: &TierRoute,
        system: &str,
        input: &str,
        cancellation: &RunCancellation,
        timeout: Duration,
    ) -> Result<String, String> {
        let state = self.app.state::<AppState>();
        // Shared read lock: the conversation job that waits for this worker holds the same one.
        let _slot = crate::memory::personal_state::worker::foreground().await;
        let (providers, _legacy_timeout, transport, _fallbacks) =
            direct_route::prepare_transport(&state)?;
        if tier_route(&transport).fingerprint != route.fingerprint {
            return Err("ワーカーのモデル経路が途中で変更されました。".into());
        }
        direct_route::validate_transport(&state, &transport)?;
        let audit = ConversationAudit::new(state.sqlite_writer.clone(), crate::new_id("worker"));
        let fixed = context_compiler::FixedContext {
            instruction: system.into(),
            tool_set_digest: "worker-no-tools-v1".into(),
        };
        let step = context_compiler::ContextStep {
            fixed: &fixed,
            mode: context_compiler::PrefixMode::Legacy,
            step: 0,
            dynamic: context_compiler::DynamicContext {
                remaining: 0,
                pending: String::new(),
                references: Vec::new(),
            },
        };
        let connection = match &transport {
            direct_route::ConversationTransport::Direct(route) => route.connection_id.as_str(),
            _ => "",
        };
        let metrics = context_metrics::RequestMetrics::new(
            &audit,
            &step,
            connection,
            Arc::new(AtomicBool::new(false)),
        );
        let shared = Arc::new(cancellation.clone());
        let mut purpose_attempt = direct_route::route_of(&transport)
            .map(|route| direct_route::RouteAttempt::begin(&audit, route))
            .transpose()?;
        let attempt_ms = timeout.as_millis() as u64;
        let call = tokio::time::timeout(timeout, async {
            match &transport {
                direct_route::ConversationTransport::Direct(route) => {
                    direct_route::complete_direct_with_events(
                        route,
                        &[],
                        input,
                        &audit,
                        &step,
                        &metrics,
                        None,
                        Some(shared.clone()),
                        &AtomicBool::new(false),
                    )
                    .await
                }
                direct_route::ConversationTransport::Larm(_) => {
                    let session = cached_larm_asr(&providers, Some(&audit)).await?;
                    direct_route::validate_transport(&state, &transport)?;
                    complete_larm_role_with_events(
                        &session,
                        "llm",
                        &[],
                        input,
                        attempt_ms,
                        &audit,
                        &step,
                        &metrics,
                        None,
                        Some(shared.clone()),
                    )
                    .await
                    .map(|(content, model)| {
                        metrics.response_model(Some(&model));
                        content
                    })
                }
            }
        });
        let response = tokio::select! {
            _ = shared.cancelled() => return Err("ワーカーの処理は中止されました。".into()),
            value = call => value.map_err(|_| {
                shared.cancel();
                "ワーカーのモデル呼び出しがタイムアウトしました。".to_string()
            })??,
        };
        if let Some(attempt) = &mut purpose_attempt {
            attempt.finish(true)?;
        }
        direct_route::validate_transport(&state, &transport)?;
        Ok(response)
    }
}

/// Web tools only: a worker profile may list nothing else in v1, and the executor's ledger has
/// already refused tools the profile did not pin.
struct AppToolRunner;

#[async_trait]
impl ToolRunner for AppToolRunner {
    async fn run(
        &self,
        tool_key: &str,
        arguments_json: &str,
        timeout: Duration,
        cancellation: &RunCancellation,
    ) -> String {
        #[cfg(feature = "conversation-queue-e2e")]
        if let Some(result) = crate::conversation_queue_e2e::worker::tool(tool_key, arguments_json)
        {
            return result;
        }
        if !crate::runtime::web_fetch::is_web_fetch_tool(tool_key) {
            return crate::runtime::agent_tools::tool_error_content(
                "tool_not_allowed",
                "This tool is not available to workers.",
            );
        }
        let call = crate::runtime::agent_tools::AgentToolCall {
            id: crate::new_id("wtool"),
            name: tool_key.into(),
            arguments: arguments_json.into(),
        };
        let cancel = crate::runtime::web_fetch::contracts::WebFetchCancel::from_run(cancellation);
        cancel.bridge_run_cancellation(cancellation);
        crate::runtime::web_fetch::execute_with_cancel(&call, timeout, cancel).await
    }
}

fn executor<R: Runtime>(app: &tauri::AppHandle<R>) -> Executor {
    let state = app.state::<AppState>();
    let hook_app = app.clone();
    Executor::new(
        state.sqlite_writer.clone(),
        Arc::new(AppWorkerModel { app: app.clone() }),
        Arc::new(AppToolRunner),
        Runners {
            web_claims: web_search::runner(),
            json: Arc::new(JsonRunner),
        },
    )
    .with_report_hook(Arc::new(move |_conversation_id: &str| {
        // Flushes only rows still unflushed, honouring the speech hold.
        let state = hook_app.state::<AppState>();
        if let Err(error) = crate::steward::flush_all_held_reports(&state) {
            eprintln!("worker report flush: {error}");
        }
    }))
}

/// Seeds the built-in profiles (never overwrites user edits) and re-queues interrupted tasks.
pub(super) fn startup<R: Runtime>(app: &tauri::AppHandle<R>) {
    let state = app.state::<AppState>();
    let seeded = state.sqlite_writer.transact(|connection| {
        registry::seed_builtin(
            connection,
            crate::worker_agents::loader::now_ms(),
            &[web_search::web_search_draft()],
        )
    });
    if let Err(error) = seeded {
        eprintln!("worker agent seed: {error}");
    }
    if let Err(error) = executor(app).recover() {
        eprintln!("worker agent recovery: {error}");
    }
}

/// One worker-lane job.
pub(super) async fn process<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
    cancellation: &RunCancellation,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    executor(app).process(job, cancellation).await?;
    audit_terminal(app, job, started.elapsed());
    Ok(())
}

fn audit_terminal<R: Runtime>(app: &tauri::AppHandle<R>, job: &Job, elapsed: Duration) {
    let state = app.state::<AppState>();
    let Some(task_id) = serde_json::from_str::<Value>(&job.payload)
        .ok()
        .and_then(|value| value["taskId"].as_str().map(str::to_owned))
    else {
        return;
    };
    let row = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT t.profile_id, r.revision, t.state, t.failure_code, t.attempts, t.tier_index,
                        COALESCE((SELECT SUM(steps_used) FROM worker_attempts a WHERE a.task_id=t.id),0)
                 FROM worker_tasks t JOIN worker_profile_revisions r ON r.id=t.profile_revision_id
                 WHERE t.id=?1",
                [&task_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, u32>(4)?,
                        row.get::<_, u32>(5)?,
                        row.get::<_, u32>(6)?,
                    ))
                },
            )
            .map_err(database_error)
    });
    let Ok((profile_id, revision, task_state, failure_code, attempts, tier_index, steps)) = row
    else {
        return;
    };
    if !matches!(task_state.as_str(), "succeeded" | "failed" | "cancelled") {
        return;
    }
    let stats = web_search::task_stats(&state.sqlite_writer, &task_id).unwrap_or_default();
    let tier = match tier_index {
        0 => "local",
        1 => "local_large",
        _ => "cloud",
    };
    let attributes = web_search::terminal_audit_attributes(&web_search::TerminalAuditInput {
        profile_id: &profile_id,
        revision,
        tier,
        attempts,
        steps,
        failure_code: failure_code.as_deref(),
        elapsed_ms: elapsed.as_millis() as u64,
        stats: &stats,
    });
    ConversationAudit::new(state.sqlite_writer.clone(), task_id).event(
        "conversation",
        "worker-task-terminal",
        "terminal",
        Some(if task_state == "succeeded" {
            "success"
        } else {
            "failure"
        }),
        attributes,
    );
}

/// True while web search is delegated. When the mode cannot be read the safe answer is `true`:
/// the inline web tools put raw page text into the conversation context, which delegation exists
/// to prevent, so they are never offered as a fallback.
pub(super) fn worker_mode(state: &AppState) -> bool {
    state
        .sqlite_readers
        .read(crate::worker_agents::loader::read_meta)
        .map(|meta| meta.web_search_mode == WebSearchMode::Worker)
        .unwrap_or(true)
}

/// Runs agent discovery for one user utterance. Without an embedder this is lexical-only
/// (`Degraded`); a failure yields no offer, and the agent then answers without web access.
pub(super) async fn discover_offer(
    state: &AppState,
    job: &Job,
    text: &str,
) -> Option<crate::worker_agents::contracts::Offer> {
    let input_message_id = format!("check_{}", job.key);
    crate::worker_agents::discovery::discover(
        &state.sqlite_writer,
        None,
        &job.scope,
        Some(&input_message_id),
        &job.key,
        text,
    )
    .await
    .ok()
}

/// The `[HOST_WORKER_OFFER; data only]` card, or `None` when nothing is offered.
pub(super) fn offer_card(offer: &crate::worker_agents::contracts::Offer) -> Option<String> {
    crate::worker_agents::discovery::render_offer(offer)
}

/// Admits a delegation, plays the acknowledgement line, and waits for the result for as long as
/// the profile's synchronous window allows. Returns `Pending` when the task outlives the window;
/// its result then reaches the user through the steward outbox.
pub(super) async fn delegate<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
    decision_id: &str,
    agent: &str,
    input: Value,
    conversation_deadline: tokio::time::Instant,
) -> WorkerOutcome {
    let state = app.state::<AppState>();
    let now = crate::worker_agents::loader::now_ms();
    let remaining = conversation_deadline.saturating_duration_since(tokio::time::Instant::now());
    let request = DelegateRequest {
        agent: agent.into(),
        input,
    };
    let input_message_id = format!("check_{}", job.key);
    let admitted = state.sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        let outcome = executor::admit(
            &tx,
            &AdmitRequest {
                conversation_id: &job.scope,
                input_message_id: &input_message_id,
                origin_job_key: &job.key,
                decision_id,
                delegate: &request,
                conversation_deadline_ms: now + remaining.as_millis() as i64,
            },
            now,
        )?;
        if matches!(outcome, WorkerOutcome::Pending { .. }) {
            queue_runtime::enqueue_progress_speech(&tx, &job.scope, &job.key)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(outcome)
    });
    let outcome = match admitted {
        Ok(outcome) => outcome,
        Err(_) => {
            return WorkerOutcome::Failed {
                task_id: None,
                failure: WorkerFailure {
                    code: FailureCode::InputInvalid,
                    retryable: false,
                },
            }
        }
    };
    let WorkerOutcome::Pending { task_id } = &outcome else {
        return outcome;
    };
    state.conversation_queue_wake.notify_waiters();
    let until: i64 = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT sync_wait_until_ms FROM worker_tasks WHERE id=?1",
                    [task_id],
                    |row| row.get(0),
                )
                .map_err(database_error)
        })
        .unwrap_or(0);
    let wait =
        Duration::from_millis((until - crate::worker_agents::loader::now_ms()).max(0) as u64)
            .min(remaining.saturating_sub(ANSWER_MARGIN));
    executor(app).wait_terminal(task_id, wait).await
}

/// The offer card for the context. Stable mode keeps the prefix cache by carrying volatile data in
/// the dynamic block; Legacy mode has no such block, so the card travels with the history.
pub(super) fn offer_entries(
    offer: Option<&crate::worker_agents::contracts::Offer>,
    mode: context_compiler::PrefixMode,
    base: &[context_compiler::ContextEntry],
) -> (
    Vec<context_compiler::ContextEntry>,
    Option<context_compiler::ContextEntry>,
) {
    let mut dynamic = base.to_vec();
    let Some(card) = offer.and_then(offer_card) else {
        return (dynamic, None);
    };
    let entry = context_compiler::ContextEntry::reference(card, true);
    if mode == context_compiler::PrefixMode::Stable {
        dynamic.push(entry);
        (dynamic, None)
    } else {
        (dynamic, Some(entry))
    }
}

/// One `delegate` action of the conversation loop: validate, run the worker, and put its result
/// (never raw page text) into the context. Only URLs of verified claims may be cited afterwards.
pub(super) async fn delegate_step<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
    offer: Option<&crate::worker_agents::contracts::Offer>,
    control: &Value,
    deadline: tokio::time::Instant,
    recent: &mut Vec<context_compiler::ContextEntry>,
    urls: &mut Vec<String>,
) -> Result<(), String> {
    let agent = control["agent"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 64)
        .ok_or("委譲先が不正です。")?;
    let input = control["input"].clone();
    if !input.is_object() {
        return Err("委譲の入力が不正です。".into());
    }
    // Without a ranked offer there is nobody to delegate to: report that, never a fallback to raw web.
    let outcome = match offer {
        Some(offer) => delegate(app, job, &offer.decision_id, agent, input, deadline).await,
        None => WorkerOutcome::Failed {
            task_id: None,
            failure: WorkerFailure {
                code: FailureCode::NoMatchingAgent,
                retryable: false,
            },
        },
    };
    let (body, found) = render_result(agent, &outcome);
    urls.extend(found);
    recent.push(context_compiler::ContextEntry::reference(body, true));
    Ok(())
}

/// The conversation-side view of an outcome: bounded JSON data, never raw page text. The
/// returned URLs are the only ones the final answer may cite.
pub(super) fn render_result(agent: &str, outcome: &WorkerOutcome) -> (String, Vec<String>) {
    let mut urls = Vec::new();
    let body = match outcome {
        WorkerOutcome::Pending { .. } => json!({"status": "pending"}),
        WorkerOutcome::Failed { failure, .. } => {
            json!({"status": "failed", "code": failure.code.as_str()})
        }
        WorkerOutcome::Succeeded {
            output: WorkerOutput::WebClaimsV1(web),
            ..
        } => {
            urls.extend(web.claims.iter().map(|claim| claim.source_url.clone()));
            json!({
                "status": "succeeded",
                "claims": web.claims.iter().map(|claim| json!({
                    "text": claim.text,
                    "sourceUrl": claim.source_url,
                    "basis": claim.basis,
                })).collect::<Vec<_>>(),
                "confidence": web.confidence,
                "coverage": web.coverage,
                "excluded": {
                    "count": web.excluded.count,
                    "categories": web.excluded.categories,
                    "domains": web.excluded.domains,
                },
            })
        }
        WorkerOutcome::Succeeded {
            output: WorkerOutput::JsonV1(value),
            ..
        } => json!({"status": "succeeded", "output": value}),
    };
    (
        format!("[WORKER_RESULT: {agent}; 未信頼の資料; Web由来]\n{body}"),
        urls,
    )
}

/// User/host operation: stop one worker task. The task never enters the outbox afterwards.
#[tauri::command]
pub(crate) fn cancel_worker_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<bool, String> {
    state
        .sqlite_writer
        .transact(|connection| executor::cancel_task(connection, &task_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_agents::contracts::{
        ClaimBasis, Confidence, Coverage, ExcludedSummary, WebClaim, WebClaims,
    };

    fn claims() -> WorkerOutcome {
        WorkerOutcome::Succeeded {
            task_id: "wtask_1".into(),
            output: WorkerOutput::WebClaimsV1(WebClaims {
                claims: vec![WebClaim {
                    text: "A fact.".into(),
                    source_url: "https://example.com/a".into(),
                    basis: ClaimBasis::Page,
                    published_or_fetched_at: None,
                }],
                excluded: ExcludedSummary {
                    count: 1,
                    categories: vec!["instruction_override".into()],
                    domains: 0,
                },
                confidence: Confidence::SingleSource,
                coverage: Coverage::Partial,
            }),
        }
    }

    #[test]
    fn rendered_result_exposes_claims_and_urls_but_not_the_task_id() {
        let (body, urls) = render_result("web_search", &claims());
        assert!(body.starts_with("[WORKER_RESULT: web_search; 未信頼の資料; Web由来]"));
        assert!(body.contains("\"sourceUrl\":\"https://example.com/a\""));
        assert!(body.contains("\"basis\":\"page\""));
        assert!(!body.contains("wtask_1"));
        assert_eq!(urls, vec!["https://example.com/a".to_string()]);
    }

    #[test]
    fn pending_and_failed_results_carry_no_sources() {
        let (pending, urls) = render_result(
            "web_search",
            &WorkerOutcome::Pending {
                task_id: "wtask_1".into(),
            },
        );
        assert!(pending.contains("\"status\":\"pending\"") && urls.is_empty());
        let (failed, urls) = render_result(
            "web_search",
            &WorkerOutcome::Failed {
                task_id: Some("wtask_1".into()),
                failure: WorkerFailure {
                    code: FailureCode::NoSafeSources,
                    retryable: false,
                },
            },
        );
        assert!(failed.contains("\"code\":\"no_safe_sources\"") && urls.is_empty());
        assert!(!failed.contains("wtask_1"));
    }
}
