use super::*;
use crate::runtime::event_hub::RuntimeEventSender;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<RuntimeEvent>>>);
impl RuntimeEventSender for Sink {
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

async fn request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    loop {
        let mut b = [0; 4096];
        let n = socket.read(&mut b).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&b[..n]);
        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
            let size = head
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|v| v.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + size {
                let body = if size == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[end + 4..end + 4 + size]).unwrap()
                };
                return (head, body);
            }
        }
    }
}
async fn respond(socket: &mut tokio::net::TcpStream, mime: &str, body: &str) {
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}

#[tokio::test]
async fn sse_generates_edits_saves_searches_and_reopens_without_control_deltas() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut marker = String::new();
        let mut instance = Value::Null;
        for round in 0..7 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (head, body) = request(&mut socket).await;
            assert!(head.starts_with("POST /v1/agents/sessions/ags_test/turns "));
            let input: Value =
                serde_json::from_str(body["input"][0]["text"].as_str().unwrap()).unwrap();
            if round == 0 {
                assert_eq!(input["tools"].as_array().unwrap().len(), 5);
                let instructions = input["instructions"].as_str().unwrap();
                let start = instructions.find("<saaa-ui-").unwrap();
                let end = instructions[start..].find('>').unwrap() + start + 1;
                marker = instructions[start..end].into();
            } else {
                assert!(
                    input["toolResult"]["result"]["result"]["error"].is_null(),
                    "{input}"
                );
            }
            let result = if round == 0 {
                &input["result"]["result"]
            } else {
                &input["toolResult"]["result"]["result"]
            };
            let call = match round {
                0 => {
                    json!({"name":"present_ui","arguments":{"definition":{"kind":"Metric","args":["runtime.summary","running","実行中"]},"summary":"実行中の件数","mode":"live"}})
                }
                1 => {
                    instance = result["instanceId"].clone();
                    assert!(instance.is_string());
                    json!({"name":"get_ui","arguments":{"instanceId":instance}})
                }
                2 => {
                    assert_eq!(result["instances"][0]["instanceId"], instance);
                    json!({"name":"present_ui","arguments":{"baseInstanceId":instance,"definition":{"kind":"Metric","args":["runtime.summary","total","総件数"]},"summary":"総件数に変更","mode":"live"}})
                }
                3 => {
                    instance = result["instanceId"].clone();
                    json!({"name":"save_ui","arguments":{"instanceId":instance,"name":"SSE試用","description":"保存した表示","tags":["SSE"]}})
                }
                4 => json!({"name":"search_ui","arguments":{"query":"SSE試用"}}),
                5 => {
                    let view = &result["views"][0]["id"];
                    assert!(view.is_string(), "{result}");
                    json!({"name":"open_ui","arguments":{"viewId":view}})
                }
                _ => {
                    assert!(result["instanceId"].is_string());
                    Value::Null
                }
            };
            respond(
                &mut socket,
                "application/json",
                &json!({"id":format!("agt_{round}"),"session_id":"ags_test"}).to_string(),
            )
            .await;
            let (mut socket, _) = listener.accept().await.unwrap();
            let (head, _) = request(&mut socket).await;
            assert!(head.starts_with("GET /v1/agents/sessions/ags_test/events "));
            if round > 0 {
                assert!(
                    head.to_lowercase()
                        .contains(&format!("last-event-id: end{}", round - 1)),
                    "{head}"
                );
            }
            let output = if call.is_null() {
                "生成・編集・保存・再利用が完了しました。".into()
            } else {
                format!("{marker}{call}</saaa-ui>")
            };
            let mut events = String::new();
            // Separate every Unicode scalar into an SSE event to exercise marker/JSON boundaries.
            for (i, ch) in output.chars().enumerate() {
                let cursor = format!("r{round}_{i}");
                events.push_str(&format!("id: {cursor}\ndata: {}\n\n",json!({"type":"message.delta","session_id":"ags_test","turn_id":format!("agt_{round}"),"cursor":cursor,"data":{"text":ch.to_string()}})));
            }
            events.push_str(&format!("id: end{round}\ndata: {}\n\n",json!({"type":"turn.completed","session_id":"ags_test","turn_id":format!("agt_{round}"),"cursor":format!("end{round}"),"data":{}})));
            respond(&mut socket, "text/event-stream", &events).await;
        }
    });
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute("UPDATE ui_settings SET enabled=1", []).unwrap();
    c.execute("INSERT INTO conversation_messages VALUES('sse-source',?1,'user','表示して編集して保存して再利用','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('sse_fixture',?1,'conversation.respond','running','sse-source','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let state = crate::test_support::app_state(c);
    let persistence_id = crate::begin_provider_session(
        &state,
        "sse_fixture",
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    let input = crate::StartTurnInput {
        run_id: "sse_fixture".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: "表示して編集して保存して再利用".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let sink = Sink::default();
    let provider = AgentSessionProviderSettings {
        id: "fixture".into(),
        enabled: true,
        label: "fixture".into(),
        location: "local".into(),
        base_url: base.clone(),
        model: "muse/fixture".into(),
        models_path: "/v1/agents/models?runtime=muse".into(),
        sessions_path: "/v1/agents/sessions".into(),
        authentication: "none".into(),
    };
    let session = SessionResponse {
        id: "ags_test".into(),
        events_url: None,
    };
    let history = [ConversationMessage {
        parts: None,
        id: "sse-source".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: "1".into(),
    }];
    let outcome = run_agent_session_sse(
        &Client::new(),
        &provider,
        &session,
        Url::parse(&format!("{base}/v1/agents/sessions/ags_test/events")).unwrap(),
        &history,
        10_000,
        None,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 1000,
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(crate::RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &persistence_id,
            }),
        },
    )
    .await;
    assert!(
        matches!(outcome, ProviderAttemptOutcome::Completed { .. }),
        "{outcome:?}"
    );
    server.await.unwrap();
    let events = sink.0.lock().unwrap();
    let text: String = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::Delta { text, .. } = e {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(text, "生成・編集・保存・再利用が完了しました。");
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e,RuntimeEvent::Activity {kind,..} if kind=="ui-presented"))
            .count(),
        3
    );
    state
        .sqlite_readers
        .read(|c| {
            let count: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM ui_tool_results WHERE run_id='sse_fixture'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 6);
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn cancellation_and_timeout_bound_the_initial_post() {
    for cancel in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let cancellation = Arc::new(crate::RunCancellation::default());
        let signal = cancellation.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = request(&mut socket).await;
            if cancel {
                signal.cancel();
            }
            std::future::pending::<()>().await;
        });
        let provider = AgentSessionProviderSettings {
            id: "fixture".into(),
            enabled: true,
            label: "fixture".into(),
            location: "local".into(),
            base_url: base.clone(),
            model: "muse/fixture".into(),
            models_path: "/v1/agents/models?runtime=muse".into(),
            sessions_path: "/v1/agents/sessions".into(),
            authentication: "none".into(),
        };
        let session = SessionResponse {
            id: "ags_test".into(),
            events_url: None,
        };
        let input = crate::StartTurnInput {
            run_id: "sse_cancel".into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: "hello".into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let sink = Sink::default();
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            run_agent_session_sse(
                &Client::new(),
                &provider,
                &session,
                Url::parse(&format!("{base}/events")).unwrap(),
                &[],
                if cancel { 1000 } else { 50 },
                None,
                ModelStreamContext {
                    reasoning_effort: "provider-default",
                    max_output_tokens: 1000,
                    input: &input,
                    on_event: &sink,
                    cancellation,
                    context_health: "green",
                    context_sources: &[],
                    context_omissions: &[],
                    output_persistence: None,
                },
            ),
        )
        .await
        .expect("initial POST must be bounded");
        if cancel {
            assert!(matches!(
                outcome,
                ProviderAttemptOutcome::Cancelled {
                    output_started: false,
                    ..
                }
            ));
        } else {
            assert!(matches!(
                outcome,
                ProviderAttemptOutcome::Failed {
                    kind: ProviderFailureKind::Timeout,
                    output_started: false,
                    ..
                }
            ));
        }
        assert!(sink.0.lock().unwrap().is_empty());
        server.abort();
    }
}

/// Uses the actual configured Muse transport against a disposable local database.
/// Run explicitly after the provider's quota is available; never touches the app's data.
#[cfg(not(coverage))]
#[tokio::test]
#[ignore = "requires live Muse subscription quota; set SAAA_LIVE_SSE=1"]
async fn live_muse_sse_ui_workflow() {
    assert_eq!(std::env::var("SAAA_LIVE_SSE").as_deref(), Ok("1"));
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute("UPDATE ui_settings SET enabled=1", []).unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('sse_live',?1,'conversation.respond','running','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let state = crate::test_support::app_state(c);
    let persistence_id = crate::begin_provider_session(
        &state,
        "sse_live",
        "muse-agent-session",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    let provider = AgentSessionProviderSettings {
        id: "muse-agent-session".into(),
        enabled: true,
        label: "Muse".into(),
        location: "local".into(),
        base_url: "http://127.0.0.1:44449".into(),
        model: "muse/muse-spark-1.3-contributor".into(),
        models_path: "/v1/agents/models?runtime=muse".into(),
        sessions_path: "/v1/agents/sessions".into(),
        authentication: "none".into(),
    };
    let prompt = "UIツールの動作を試します。実行中の件数をMetricで表示してください。次にget_uiで取得し、総件数表示に編集してください。それを『SSE試用』という名前で保存し、search_uiで検索してopen_uiで再表示するところまで実行してください。数値を作らず、提供されたツールを使ってください。";
    let input = crate::StartTurnInput {
        run_id: "sse_live".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: prompt.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let history = [ConversationMessage {
        id: "synthetic".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        role: "user".into(),
        content: prompt.into(),
        created_at: "1".into(),
        parts: None,
    }];
    let sink = Sink::default();
    let started = Instant::now();
    let outcome = super::super::stream_agent_session_provider(
        &provider,
        &history,
        180_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 4000,
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(crate::RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &persistence_id,
            }),
        },
    )
    .await;
    let results = state.sqlite_readers.read(|c| {
        let mut stmt = c.prepare("SELECT result_json FROM ui_tool_results WHERE run_id='sse_live' ORDER BY rowid").unwrap();
        let rows = stmt.query_map([],|r|r.get::<_,String>(0)).unwrap().map(|r|serde_json::from_str::<Value>(&r.unwrap()).unwrap()).collect::<Vec<_>>();
        Ok(rows)
    }).unwrap();
    let evidence = json!({"model":provider.model,"transport":"AgentSession SSE","elapsedMs":started.elapsed().as_millis(),"outcome":format!("{outcome:?}"),"toolResults":results});
    println!("{evidence}");
    if let Ok(path) = std::env::var("SAAA_LIVE_SSE_EVIDENCE") {
        std::fs::write(path, serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
    }
    assert!(
        matches!(outcome, ProviderAttemptOutcome::Completed { .. }),
        "Live provider did not complete: {outcome:?}"
    );
    assert!(results.len() >= 6, "Workflow incomplete: {evidence}");
    assert_eq!(
        results
            .iter()
            .filter(|r| r["messageId"].is_string())
            .count(),
        3
    );
    assert!(results
        .iter()
        .any(|r| r["views"].as_array().is_some_and(|v| !v.is_empty())));
    let events = sink.0.lock().unwrap();
    assert!(!events
        .iter()
        .any(|e| matches!(e,RuntimeEvent::Delta {text,..} if text.contains("saaa-ui-"))));
}

#[path = "coding_workflow_tests.rs"]
mod coding_workflow_tests;
