use crate::ipc_contract::RuntimeEvent;
use crate::runtime::event_hub::RuntimeEventSender;
use std::sync::{Arc, Mutex};

struct Sink {
    acks: Mutex<Vec<String>>,
    holds: Mutex<Vec<String>>,
    deltas: Mutex<Vec<String>>,
    order: Mutex<Vec<&'static str>>,
}

impl RuntimeEventSender for Sink {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        match event {
            RuntimeEvent::Delta { text, .. } => {
                self.deltas.lock().unwrap().push(text);
                self.order.lock().unwrap().push("delta");
            }
            RuntimeEvent::Started { .. } => {
                self.order.lock().unwrap().push("thinking");
            }
            RuntimeEvent::MessageCommitted { .. } => {
                self.order.lock().unwrap().push("committed");
            }
            RuntimeEvent::MessageCompleted { .. } => {
                self.order.lock().unwrap().push("answer");
            }
            _ => {}
        }
        Ok(())
    }
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(Sink {
            acks: Mutex::new(self.acks.lock().unwrap().clone()),
            holds: Mutex::new(self.holds.lock().unwrap().clone()),
            deltas: Mutex::new(self.deltas.lock().unwrap().clone()),
            order: Mutex::new(self.order.lock().unwrap().clone()),
        })
    }
    fn acknowledge_text<'a>(
        &'a self,
        _state: &'a crate::AppState,
        _run_id: &'a str,
        _conversation_id: &'a str,
        text: String,
        _cancellation: Arc<crate::RunCancellation>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        self.acks.lock().unwrap().push(text);
        self.order.lock().unwrap().push("ack");
        Box::pin(async {})
    }
    fn acknowledge_hold<'a>(
        &'a self,
        _state: &'a crate::AppState,
        _run_id: &'a str,
        _conversation_id: &'a str,
        text: String,
        _cancellation: Arc<crate::RunCancellation>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        self.holds.lock().unwrap().push(text);
        Box::pin(async {})
    }
}

fn sink() -> Sink {
    Sink {
        acks: Mutex::new(vec![]),
        holds: Mutex::new(vec![]),
        deltas: Mutex::new(vec![]),
        order: Mutex::new(vec![]),
    }
}

struct Turn {
    message: String,
    steps: Vec<(i64, String, String, String)>,
    bodies: Vec<serde_json::Value>,
    hits: Vec<String>,
    sink: Sink,
}

async fn run_turn(run_id: &str, origin: &str, transition: &'static str, content: &str) -> Turn {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let h = Harness::new();
    let route = if origin == "voice" { "butler" } else { "dynamic-lan" };
    let (fake, server) = if origin == "voice" {
        Fake::start(route, transition, h.fixture.clock.clone()).await
    } else {
        std::env::set_var("LARM_API_TOKEN", "test-control-token");
        Fake::listen("127.0.0.1:9810", route, transition, h.fixture.clock.clone()).await
    };
    h.state.sqlite_writer.write(|connection| {
        let mut providers = crate::persistence::load_model_providers(connection)?;
        providers.harness.address = fake.base.clone();
        providers.harness.larm_profile = Some("fixture-voice".into());
        if origin != "voice" {
            for provider in &mut providers.providers {
                if let crate::ModelProviderSettings::DynamicLan(provider) = provider {
                    provider.host = "127.0.0.1".into();
                }
            }
        }
        let providers_json = serde_json::to_string(&providers).map_err(|error| error.to_string())?;
        connection.execute(
            "UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.model' AND key='default'",
            [providers_json],
        ).map_err(|error| error.to_string())?;
        if transition == "frontend-slow" {
            let mut roles = crate::persistence::load_role_routing_settings(connection)?;
            roles.limits.frontend_timeout_ms = 200;
            let roles_json = serde_json::to_string(&roles).map_err(|error| error.to_string())?;
            connection.execute(
                "UPDATE settings_documents SET value_json=?1 WHERE namespace='routing.roles' AND key='default'",
                [roles_json],
            ).map_err(|error| error.to_string())?;
        }
        Ok(())
    }).unwrap();
    let (cancel, _) = watch::channel(false);
    *OWNER.lock().await = Some(Arc::new(Owner {
        id: format!("butler-{run_id}"),
        conversation: crate::PRIMARY_CONVERSATION_ID.into(),
        base: fake.base.clone(),
        profile: "fixture-voice".into(),
        cancel,
        ready: OnceCell::new(),
        started: AtomicBool::new(false),
        lease_key: current_lease_key(&h.state.sqlite_writer).unwrap(),
        sqlite_writer: h.state.sqlite_writer.clone(),
    }));
    let input = crate::StartTurnInput {
        run_id: run_id.into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: content.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: vec![],
        input_origin: origin.into(),
        presentation_mode: "visual".into(),
    };
    crate::runtime::turns::prepare_runtime_run(&h.state, &input).unwrap();
    let events = sink();
    let message = crate::runtime::conversation_turn::execute_conversation_turn_with_candidates(
        &h.state,
        &input,
        &events,
        Arc::new(crate::RunCancellation::default()),
        Vec::new(),
    )
    .await
    .unwrap_or_else(|error| panic!("{run_id} failed: {error:?}"));
    crate::runtime::voice_response::complete(
        &h.state,
        &input,
        &events,
        Arc::new(crate::RunCancellation::default()),
        &message,
    )
    .await
    .unwrap();
    let steps = h.state.sqlite_readers.read(|connection| {
        let mut statement = connection.prepare(
            "SELECT ordinal, purpose, actor_id, status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal",
        ).map_err(|error| error.to_string())?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        }).map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())
    }).unwrap();
    let bodies = fake.bodies.lock().unwrap().clone();
    let hits = fake.hits.lock().unwrap().clone();
    shutdown().await;
    server.abort();
    Turn {
        message: message.content,
        steps,
        bodies,
        hits,
        sink: events,
    }
}

#[tokio::test]
async fn role_routed_voice_turn_reaches_larm_for_every_step() {
    let turn = run_turn("run_butler_both", "voice", "ready", "u").await;
    assert_eq!(
        turn.steps,
        vec![
            (0, "frontend".into(), "larm-frontdesk".into(), "succeeded".into()),
            (1, "respond".into(), "larm-reasoner".into(), "succeeded".into()),
        ]
    );
    assert_eq!(
        turn.hits.iter().filter(|hit| hit.as_str() == "backchannel").count(),
        1,
        "hits: {:?}",
        turn.hits
    );
    assert_eq!(
        turn.hits.iter().filter(|hit| hit.as_str() == "llm").count(),
        1,
        "hits: {:?}",
        turn.hits
    );
    assert_eq!(turn.message, "ornith-answer");
}

#[tokio::test]
async fn voice_turn_runs_frontend_then_reasoner() {
    let turn = run_turn("run_butler_voice", "voice", "ready", "u").await;
    let backchannel = turn.bodies.iter().find(|body| {
        body["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| message["content"] == crate::role_routing::frontend::INSTRUCTION))
    });
    let backchannel = backchannel.expect("backchannel request");
    assert_eq!(backchannel["model"], "qwen3.5-2b-fast-response");
    assert_eq!(backchannel["stream"], true);
    assert_eq!(
        backchannel["chat_template_kwargs"],
        json!({"enable_thinking": false})
    );
    let reasoner = turn
        .bodies
        .iter()
        .find(|body| body["model"] == "ornith-1.5-35b")
        .expect("reasoner request");
    assert_eq!(reasoner["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(
        turn.sink.acks.lock().unwrap().as_slice(),
        [crate::role_routing::frontend::WAIT_LINE]
    );
    assert!(turn.sink.order.lock().unwrap().contains(&"committed"));
    assert_eq!(turn.message, "ornith-answer");
    assert!(turn.hits.contains(&"backchannel".into()));
    assert!(turn.hits.contains(&"llm".into()));
}

#[tokio::test]
async fn greeting_finishes_without_reasoner() {
    let turn = run_turn("run_butler_greet", "voice", "frontend-greeting", "u").await;
    assert_eq!(turn.sink.acks.lock().unwrap().as_slice(), ["x"]);
    assert_eq!(turn.message, "x");
    let order = turn.sink.order.lock().unwrap().clone();
    let thinking_at = order.iter().position(|event| *event == "thinking").expect("thinking");
    let committed_at = order.iter().position(|event| *event == "committed").expect("committed");
    assert!(thinking_at < committed_at, "order: {order:?}");
    assert!(!turn.hits.contains(&"llm".into()), "hits: {:?}", turn.hits);
    assert_eq!(turn.steps[0].3, "succeeded");
    assert_eq!(turn.steps[1].3, "cancelled");
}

#[tokio::test]
async fn nod_finishes_without_reasoner() {
    let turn = run_turn("run_butler_nod", "voice", "frontend-nod", "u").await;
    assert_eq!(turn.sink.acks.lock().unwrap().as_slice(), ["x"]);
    assert_eq!(turn.message, "x");
    assert!(!turn.hits.contains(&"llm".into()), "hits: {:?}", turn.hits);
    assert_eq!(turn.steps[0].3, "succeeded");
    assert_eq!(turn.steps[1].3, "cancelled");
}

#[tokio::test]
async fn thanks_finishes_without_reasoner() {
    let turn = run_turn("run_butler_thanks", "voice", "frontend-thanks", "u").await;
    assert_eq!(turn.sink.acks.lock().unwrap().as_slice(), ["x"]);
    assert_eq!(turn.message, "x");
    assert!(!turn.hits.contains(&"llm".into()), "hits: {:?}", turn.hits);
    assert_eq!(turn.steps[0].3, "succeeded");
    assert_eq!(turn.steps[1].3, "cancelled");
}

#[tokio::test]
async fn low_confidence_ack_still_runs_reasoner() {
    let turn = run_turn("run_butler_low", "voice", "frontend-low", "u").await;
    assert_eq!(
        turn.sink.acks.lock().unwrap().as_slice(),
        [crate::role_routing::frontend::WAIT_LINE]
    );
    assert_eq!(turn.message, "ornith-answer");
    assert!(turn.bodies.len() >= 2, "bodies: {:?}", turn.bodies);
    assert_eq!(turn.steps[1].3, "succeeded");
}

#[tokio::test]
async fn handoff_fixture_still_runs_reasoner() {
    let turn = run_turn("run_butler_mixed", "voice", "ready", "u").await;
    assert_eq!(turn.message, "ornith-answer");
    assert!(turn.bodies.len() >= 2, "bodies: {:?}", turn.bodies);
    assert_eq!(turn.steps[1].3, "succeeded");
}

#[tokio::test]
async fn frontend_failure_still_runs_reasoner() {
    let turn = run_turn("run_butler_bad", "voice", "frontend-bad", "u").await;
    assert!(turn.sink.acks.lock().unwrap().is_empty());
    assert_eq!(turn.message, "ornith-answer");
    assert_eq!(turn.steps[1].3, "succeeded");
}

#[tokio::test]
async fn frontend_timeout_still_runs_reasoner() {
    let turn = run_turn("run_butler_timeout", "voice", "frontend-slow", "u").await;
    assert!(turn.sink.acks.lock().unwrap().is_empty());
    assert_eq!(turn.message, "ornith-answer");
}

#[tokio::test]
async fn text_turn_skips_frontend_provider() {
    let turn = run_turn("run_butler_text", "text", "ready", "u").await;
    assert!(turn
        .bodies
        .iter()
        .all(|body| body["messages"].as_array().is_none_or(|messages| {
            messages
                .iter()
                .all(|message| message["content"] != crate::role_routing::frontend::INSTRUCTION)
        })));
    assert_eq!(turn.message, "fixture");
    assert!(turn.sink.acks.lock().unwrap().is_empty());
    assert_eq!(turn.hits, ["llm".to_string()], "hits: {:?}", turn.hits);
    assert!(
        turn.bodies.iter().any(|body| body["model"] == "ornith-1.5-35b"),
        "bodies: {:?}",
        turn.bodies
    );
    assert!(turn.hits.iter().all(|hit| hit != "backchannel"));
    assert_eq!(turn.steps[0].2, "larm-frontdesk");
    assert_eq!(turn.steps[1].2, "larm-reasoner");
}

#[tokio::test]
async fn frontend_output_is_not_forwarded_to_reasoner() {
    let turn = run_turn("run_butler_forward", "voice", "ready", "u").await;
    let reasoner = turn.bodies.iter().find(|body| {
        body["messages"].as_array().is_some_and(|messages| {
            messages.iter().any(|message| message["content"] == "ornith-answer")
                || messages.iter().all(|message| {
                    message["content"] != crate::role_routing::frontend::INSTRUCTION
                })
        })
    });
    let encoded = serde_json::to_string(&turn.bodies).unwrap();
    let reasoner_requests = turn.bodies.iter().filter(|body| {
        !body["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| message["content"] == crate::role_routing::frontend::INSTRUCTION))
    });
    for body in reasoner_requests {
        let text = serde_json::to_string(body).unwrap();
        assert!(!text.contains("\"ack\":\"working\""), "{encoded}");
    }
    let _ = reasoner;
}

#[tokio::test]
async fn ack_is_spoken_through_outer_event_hub() {
    let turn = run_turn("run_butler_ack", "voice", "ready", "u").await;
    assert_eq!(
        turn.sink.acks.lock().unwrap().as_slice(),
        [crate::role_routing::frontend::WAIT_LINE]
    );
}

#[tokio::test]
async fn ack_speech_is_queued_before_answer_speech() {
    let turn = run_turn("run_butler_order", "voice", "ready", "u").await;
    let order = turn.sink.order.lock().unwrap().clone();
    let ack_at = order.iter().position(|event| *event == "ack").expect("ack");
    let answer_at = order.iter().position(|event| *event == "answer").expect("answer");
    assert!(ack_at < answer_at, "order: {order:?}");
    assert!(order.iter().enumerate().all(|(index, event)| *event != "delta" || index > ack_at));
}

#[tokio::test]
async fn reasoner_draft_is_not_visible_before_adoption() {
    let turn = run_turn("run_butler_draft", "voice", "ready", "u").await;
    assert!(turn.sink.deltas.lock().unwrap().is_empty());
    assert_eq!(turn.message, "ornith-answer");
}

#[tokio::test(start_paused = true)]
async fn filler_is_spoken_every_fifth_tick() {
    let _turn = run_turn("run_butler_filler", "voice", "reasoner-slow", "u").await;
    let ticks = crate::role_routing::frontend::take_filler_ticks();
    assert!(ticks.starts_with(&[5, 10]), "ticks: {ticks:?}");
    assert!(ticks.iter().all(|tick| tick % 5 == 0), "ticks: {ticks:?}");
}

#[tokio::test(start_paused = true)]
async fn filler_is_deferred_once_while_previous_speech_plays() {
    crate::runtime::event_hub::test_mark_speech_playing("run_butler_defer");
    crate::runtime::event_hub::test_clear_speech_at_tick("run_butler_defer", 6);
    let _turn = run_turn("run_butler_defer", "voice", "reasoner-slow", "u").await;
    let ticks = crate::role_routing::frontend::take_filler_ticks();
    assert_eq!(ticks.first().copied(), Some(6), "ticks: {ticks:?}");
    assert!(!ticks.contains(&5), "ticks: {ticks:?}");
}

#[tokio::test]
async fn no_filler_when_reasoner_finishes_within_ten_seconds() {
    let turn = run_turn("run_butler_fast", "voice", "ready", "u").await;
    assert!(turn.sink.holds.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn reasoner_times_out_when_the_step_budget_elapses() {
    let turn = run_turn("run_butler_forty", "voice", "reasoner-hang", "u").await;
    assert_eq!(turn.message, "すみません、時間内にお答えできませんでした。");
    assert_eq!(turn.steps[1].3, "cancelled");
}
