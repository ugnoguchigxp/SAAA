#![cfg(test)]
use super::*;
use crate::runtime::context::world::wire_test_support::{Harness, TRANSITIONS};
use serde_json::{json, Value};
#[path = "world_wire_fixture.rs"]
mod fixture;
use fixture::Fake;
#[tokio::test]
async fn wr_t13_shared_voice_lease_wait_refreshes_actual_http_frame() {
    matrix_case("shared-larm", "initial", false).await;
}
#[tokio::test]
async fn wr_t22_http_route_transition_matrix() {
    for route in ["openai-compatible", "dynamic-lan", "shared-larm"] {
        for transition in TRANSITIONS {
            matrix_case(route, transition, true).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "operator E2E using the running ContextStill MCP endpoint"]
async fn shared_larm_claim_context_still_tool_result_and_final_answer_are_one_flow() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let mut h = Harness::new();
    h.state.context_still_search =
        crate::memory::context_still_search::ContextStillSearchClient::from_environment();
    assert!(h.state.context_still_search.is_configured());
    let (f, server) = Fake::start("shared-larm", "context-still", h.fixture.clock.clone()).await;
    let (cancel, _) = watch::channel(false);
    *OWNER.lock().await = Some(Arc::new(Owner {
        id: "context-still-owner".into(),
        conversation: crate::PRIMARY_CONVERSATION_ID.into(),
        base: f.base.clone(),
        profile: saaa_larm_session::DEFAULT_PROFILE.into(),
        cancel,
        ready: OnceCell::new(),
        started: AtomicBool::new(false),
        lease_key: current_lease_key(&h.state.sqlite_writer).unwrap(),
        sqlite_writer: h.state.sqlite_writer.clone(),
    }));
    let input: crate::StartTurnInput = serde_json::from_value(json!({
        "runId": crate::memory::personal_state::world::runtime_test_support::RUN_ID,
        "conversationId": crate::PRIMARY_CONVERSATION_ID,
        "content": "hello",
        "inputOrigin": "voice",
        "presentationMode": "visual"
    }))
    .unwrap();
    let sink = tauri::ipc::Channel::<crate::RuntimeEvent>::new(|_| Ok(()));
    let context = crate::providers::stream::ModelStreamContext {
        reasoning_effort: "low",
        max_output_tokens: 128,
        input: &input,
        on_event: &sink,
        cancellation: Arc::default(),
        context_health: "green",
        context_sources: &h.composed.envelope.selected,
        context_omissions: &h.composed.envelope.omitted,
        output_persistence: Some(crate::ProviderOutputPersistence {
            state: &h.state,
            session_id: &h.session,
            world: h.composed.world.as_ref(),
        }),
    };
    let outcome = crate::providers::stream::stream_voice_aware_dynamic_lan_provider(
        &crate::DynamicLanProviderSettings {
            id: "fixture".into(),
            enabled: true,
            label: "fixture".into(),
            location: "local".into(),
            host: "127.0.0.1".into(),
            request_options: None,
        },
        &crate::HarnessSettings {
            address: f.base.clone(),
            larm_profile: Some(saaa_larm_session::DEFAULT_PROFILE.into()),
            tts_voice: None,
        },
        true,
        crate::PRIMARY_CONVERSATION_ID,
        &h.history,
        10_000,
        context,
    )
    .await;
    let crate::ProviderAttemptOutcome::Completed { content, .. } = outcome else {
        panic!("expected completion: {outcome:?}");
    };
    assert_eq!(content, "ContextStillの検索結果を確認しました。");
    let tool_content: Value = {
        let bodies = f.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        let content = bodies[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "tool")
            .unwrap()["content"]
            .as_str()
            .unwrap();
        serde_json::from_str(content).unwrap()
    };
    assert_eq!(tool_content["source"], "context_still");
    end("context-still-owner").await.unwrap();
    assert!(f.released.load(Ordering::SeqCst));
    let projected = crate::larm_voice::render_response(
        crate::PRIMARY_CONVERSATION_ID,
        crate::larm_voice::ResponseKind::Final,
        &content,
        "ja",
        Arc::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        projected, content,
        "backchannel must not rewrite Qwen output"
    );
    server.abort();
}
async fn matrix_case(route: &'static str, transition: &'static str, emit: bool) {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let h = Harness::new();
    h.transition(transition);
    let mut history = h.history.clone();
    let mut persistence_session = h.session.clone();
    let (f, server) = Fake::start(route, transition, h.fixture.clock.clone()).await;
    if route == "shared-larm" {
        let (cancel, _) = watch::channel(false);
        *OWNER.lock().await = Some(Arc::new(Owner {
            id: "world-owner".into(),
            conversation: crate::PRIMARY_CONVERSATION_ID.into(),
            base: f.base.clone(),
            profile: "fixture".into(),
            cancel,
            ready: OnceCell::new(),
            started: AtomicBool::new(false),
            lease_key: current_lease_key(&h.state.sqlite_writer).unwrap(),
            sqlite_writer: h.state.sqlite_writer.clone(),
        }));
    }
    let input:crate::StartTurnInput=serde_json::from_value(json!({"runId":crate::memory::personal_state::world::runtime_test_support::RUN_ID,"conversationId":crate::PRIMARY_CONVERSATION_ID,"content":"hello","inputOrigin":if route=="shared-larm" {"voice"}else{"text"},"presentationMode":"visual"})).unwrap();
    let sink = tauri::ipc::Channel::<crate::RuntimeEvent>::new(|_| Ok(()));
    macro_rules! invoke {
        () => {{
            let context = crate::providers::stream::ModelStreamContext {
                reasoning_effort: "low",
                max_output_tokens: 128,
                input: &input,
                on_event: &sink,
                cancellation: Arc::default(),
                context_health: "green",
                context_sources: &h.composed.envelope.selected,
                context_omissions: &h.composed.envelope.omitted,
                output_persistence: Some(crate::ProviderOutputPersistence {
                    state: &h.state,
                    session_id: &persistence_session,
                    world: h.composed.world.as_ref(),
                }),
            };
            let provider = crate::DynamicLanProviderSettings {
                id: "fixture".into(),
                enabled: true,
                label: "fixture".into(),
                location: "local".into(),
                host: "127.0.0.1".into(),
                request_options: None,
            };
            match route {
                "shared-larm" => {
                    crate::providers::stream::stream_voice_aware_dynamic_lan_provider(
                        &provider,
                        &crate::HarnessSettings {
                            address: f.base.clone(),
                            larm_profile: Some("fixture".into()),
                            tts_voice: None,
                        },
                        true,
                        crate::PRIMARY_CONVERSATION_ID,
                        &history,
                        10000,
                        context,
                    )
                    .await
                }
                "dynamic-lan" => {
                    let connection =
                        crate::providers::dynamic_lan::DynamicLanConnection::resolve_world_fixture(
                            url::Url::parse(&f.base).unwrap(),
                            Arc::default(),
                        )
                        .await
                        .unwrap();
                    crate::providers::stream::stream_allocated_dynamic_lan(
                        &provider,
                        &history,
                        10000,
                        connection,
                        crate::CleanupOutcome::NotStarted,
                        context,
                    )
                    .await
                }
                _ => {
                    crate::providers::stream::stream_model_provider(
                        &crate::OpenAiCompatibleProviderSettings {
                            id: "fixture".into(),
                            enabled: true,
                            label: "fixture".into(),
                            location: "local".into(),
                            endpoint: format!("{}/llm/v1", f.base),
                            model: "fixture".into(),
                            authentication: "none".into(),
                            request_options: None,
                        },
                        &history,
                        10000,
                        context,
                    )
                    .await
                }
            }
        }};
    }
    let mut outcome = invoke!();
    if transition == "fallback" {
        let crate::ProviderAttemptOutcome::Failed {
            kind,
            output_started,
            ..
        } = outcome
        else {
            panic!("expected first failure: {outcome:?}");
        };
        assert!(crate::runtime::conversation_turn::provider_fallback_allowed(kind, output_started));
        let sent = f
            .bodies
            .lock()
            .unwrap()
            .last()
            .and_then(|body| body["messages"].as_array())
            .and_then(|messages| {
                messages
                    .iter()
                    .filter_map(|m| m["content"].as_str())
                    .find(|v| v.contains("[WORLD_MODEL"))
            })
            .unwrap()
            .to_string();
        history
            .iter_mut()
            .filter(|message| message.content.contains("[WORLD_MODEL"))
            .for_each(|message| message.content = sent.clone());
        persistence_session = crate::begin_provider_session(
            &h.state,
            crate::memory::personal_state::world::runtime_test_support::RUN_ID,
            "fixture-fallback",
            "openai-compatible",
            &"c".repeat(64),
        )
        .unwrap();
        outcome = invoke!();
    } else if transition == "session-resume" {
        assert!(
            matches!(outcome, crate::ProviderAttemptOutcome::Completed { .. }),
            "{route}: {outcome:?}"
        );
        let sent = f
            .bodies
            .lock()
            .unwrap()
            .last()
            .and_then(|body| body["messages"].as_array())
            .and_then(|messages| {
                messages
                    .iter()
                    .filter_map(|m| m["content"].as_str())
                    .find(|v| v.contains("[WORLD_MODEL"))
            })
            .unwrap()
            .to_string();
        history
            .iter_mut()
            .filter(|message| message.content.contains("[WORLD_MODEL"))
            .for_each(|message| message.content = sent.clone());
        persistence_session = crate::begin_provider_session(
            &h.state,
            crate::memory::personal_state::world::runtime_test_support::RUN_ID,
            "fixture-resumed",
            "openai-compatible",
            &"b".repeat(64),
        )
        .unwrap();
        h.fixture.set_now(h.fixture.now() + 1000);
        outcome = invoke!();
    }
    if route == "shared-larm" {
        end("world-owner").await.unwrap();
    }
    server.abort();
    let denied = transition == "scope-switch";
    assert_eq!(
        matches!(outcome, crate::ProviderAttemptOutcome::Completed { .. }),
        !denied,
        "{route}/{transition}: {outcome:?}"
    );
    let bodies = f.bodies.lock().unwrap();
    let expected_requests = match transition {
        "fallback" => 4,
        "tool-continuation" | "session-resume" => 2,
        _ if denied => 0,
        _ => 1,
    };
    assert_eq!(bodies.len(), expected_requests, "{route}/{transition}");
    if emit {
        h.matrix_report(route, transition, &bodies, denied);
    } else if !denied {
        h.assert_wire(bodies.last().unwrap());
    }
    if route != "openai-compatible" {
        assert!(f.released.load(Ordering::SeqCst));
    }
}
