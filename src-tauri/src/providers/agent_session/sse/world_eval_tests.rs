#![cfg(test)]
//! WD supersedes the old M4A exclusion: AgentSession sends World only in its first turn.
use super::workflow_tests::{request, respond, Sink};
use super::*;
use crate::memory::personal_state::world::runtime_test_support::RUN_ID;
use crate::runtime::context::world::{g1_tests as graph, turn::compose_parts};
use std::sync::Arc;

fn messages(value: &Value) -> &Vec<Value> {
    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        return messages;
    }
    messages(&value["conversation"])
}

#[tokio::test]
async fn world_m4a_agent_routes() {
    let mut report = Vec::new();
    for mode in 0..3 {
        let expired = mode != 0;
        let fixture = graph::g1_fixture();
        let scope = graph::load_scope(&fixture);
        let access = fixture.access();
        let mut base = graph::window();
        base.messages.last_mut().unwrap().content = "hello".into();
        let composed = compose_parts(
            true,
            Some(Arc::new(fixture.service())),
            access.principal,
            access.policy_revision,
            Some(graph::graph_request("tech")),
            RUN_ID,
            &scope,
            base,
            vec![],
            graph::allowed(&scope),
        )
        .unwrap();
        if mode == 1 {
            fixture.set_now(fixture.now() + 1000);
        }
        let capabilities = Arc::new(
            crate::generated_capabilities::service::CapabilityService::build(
                fixture.writer.clone(),
                &std::path::PathBuf::new(),
                std::path::PathBuf::new(),
                None,
            ),
        );
        let state =
            crate::test_state::app_state_with_capabilities(fixture.writer.clone(), capabilities);
        fixture
            .writer
            .write(|c| {
                c.execute("UPDATE ui_settings SET enabled=1", [])
                    .map(|_| ())
                    .map_err(crate::database_error)
            })
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut wire = Vec::new();
            for round in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (head, body) = request(&mut socket).await;
                assert!(head.starts_with("POST "));
                let input: Value =
                    serde_json::from_str(body["input"][0]["text"].as_str().unwrap()).unwrap();
                let msgs = messages(&input);
                assert_eq!(msgs.iter().filter(|m| m["role"] == "user").count(), 1);
                let has_world = msgs.iter().any(|m| {
                    m["content"]
                        .as_str()
                        .is_some_and(|s| s.contains("[WORLD_MODEL"))
                });
                assert_eq!(has_world, !expired && round == 0);
                wire.push((body, has_world));
                let output = if round == 0 {
                    let instructions = input["instructions"].as_str().unwrap();
                    let start = instructions.find("<saaa-ui-").unwrap();
                    let end = instructions[start..].find('>').unwrap() + start + 1;
                    let call = json!({"name":"present_ui","arguments":{"definition":"root=ModelStatus(\"larm.status\")","summary":"合成","mode":"live"}});
                    format!("{}{call}</saaa-ui>", &instructions[start..end])
                } else {
                    "complete".into()
                };
                respond(
                    &mut socket,
                    "application/json",
                    &json!({"id":format!("agt_{round}"),"session_id":"ags_test"}).to_string(),
                )
                .await;
                let (mut socket, _) = listener.accept().await.unwrap();
                let (head, _) = request(&mut socket).await;
                assert!(head.starts_with("GET "));
                let event = format!(
                    "data: {}\n\ndata: {}\n\n",
                    json!({"type":"message.delta","session_id":"ags_test","turn_id":format!("agt_{round}"),"data":{"text":output}}),
                    json!({"type":"turn.completed","session_id":"ags_test","turn_id":format!("agt_{round}"),"data":{}})
                );
                respond(&mut socket, "text/event-stream", &event).await;
            }
            wire
        });
        let persistence_id = crate::begin_provider_session(
            &state,
            RUN_ID,
            "fixture",
            "openai-compatible",
            &"a".repeat(64),
        )
        .unwrap();
        let provider = AgentSessionProviderSettings {
            id: "fixture".into(),
            enabled: true,
            label: "fixture".into(),
            location: "local".into(),
            base_url: base_url.clone(),
            model: "fixture".into(),
            models_path: "/v1/agents/models".into(),
            sessions_path: "/v1/agents/sessions".into(),
            authentication: "none".into(),
        };
        let input = crate::StartTurnInput {
            run_id: RUN_ID.into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: "hello".into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: vec![],
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let history: Vec<_> = composed
            .envelope
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| ConversationMessage {
                id: format!("p{i}"),
                conversation_id: input.conversation_id.clone(),
                role: m.role.clone(),
                content: m.content.clone(),
                created_at: "1".into(),
                parts: None,
            })
            .collect();
        if mode == 2 {
            // Exercise both actual adapters with the same prepared World, then revoke it at the
            // failed primary boundary. The production route's fallback policy is checked too.
            use tokio::io::AsyncWriteExt;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
            let primary = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (_, body) = request(&mut socket).await;
                assert!(body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["content"]
                        .as_str()
                        .is_some_and(|s| s.contains("[WORLD_MODEL"))));
                socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            });
            let failed = crate::stream_model_provider(
                &crate::OpenAiCompatibleProviderSettings {
                    id: "primary".into(),
                    enabled: true,
                    label: "primary".into(),
                    location: "local".into(),
                    endpoint,
                    model: "fixture".into(),
                    authentication: "none".into(),
                    request_options: None,
                },
                &history,
                5000,
                ModelStreamContext {
                    reasoning_effort: "low",
                    max_output_tokens: 256,
                    input: &input,
                    on_event: &Sink::default(),
                    cancellation: Arc::default(),
                    context_health: "green",
                    context_sources: &composed.envelope.selected,
                    context_omissions: &composed.envelope.omitted,
                    output_persistence: Some(crate::ProviderOutputPersistence {
                        state: &state,
                        session_id: &persistence_id,
                        world: composed.world.as_ref(),
                    }),
                },
            )
            .await;
            let ProviderAttemptOutcome::Failed {
                kind,
                output_started,
                ..
            } = failed
            else {
                panic!("expected primary failure")
            };
            assert!(
                crate::runtime::conversation_turn::provider_fallback_allowed(kind, output_started)
            );
            tokio::time::timeout(Duration::from_secs(5), primary)
                .await
                .unwrap()
                .unwrap();
            fixture.set_now(fixture.now() + 1000);
        }
        let outcome = run_agent_session_sse(
            &Client::new(),
            &provider,
            &SessionResponse {
                id: "ags_test".into(),
                events_url: None,
            },
            Url::parse(&format!("{base_url}/v1/agents/sessions/ags_test/events")).unwrap(),
            &history,
            5000,
            None,
            ModelStreamContext {
                reasoning_effort: "low",
                max_output_tokens: 256,
                input: &input,
                on_event: &Sink::default(),
                cancellation: Arc::default(),
                context_health: "green",
                context_sources: &composed.envelope.selected,
                context_omissions: &composed.envelope.omitted,
                output_persistence: Some(crate::ProviderOutputPersistence {
                    state: &state,
                    session_id: &persistence_id,
                    world: composed.world.as_ref(),
                }),
            },
        )
        .await;
        assert!(
            matches!(outcome, ProviderAttemptOutcome::Completed { .. }),
            "{outcome:?}"
        );
        let bodies = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        let selected: Vec<bool> = fixture.writer.read_serialized(|c| {
            let mut s = c.prepare("SELECT EXISTS(SELECT 1 FROM context_generation_inputs i WHERE i.generation_id=g.id AND i.source_kind='world-model' AND i.selected=1) FROM context_generations g WHERE status='completed' ORDER BY ordinal").map_err(crate::database_error)?;
            let rows = s.query_map([], |r| r.get(0)).map_err(crate::database_error)?;
            rows.collect::<Result<Vec<_>,_>>().map_err(crate::database_error)
        }).unwrap();
        assert_eq!(selected, vec![!expired, false]);
        assert_eq!(bodies.len(), 2);
        let digests: Vec<String> = fixture.writer.read_serialized(|c| {
            let mut s = c.prepare("SELECT request_digest FROM context_generations WHERE status='completed' ORDER BY ordinal").map_err(crate::database_error)?;
            let rows = s.query_map([], |r| r.get(0)).map_err(crate::database_error)?;
            rows.collect::<Result<Vec<_>,_>>().map_err(crate::database_error)
        }).unwrap();
        use sha2::Digest;
        for ((body, _), digest) in bodies.iter().zip(&digests) {
            assert_eq!(
                *digest,
                format!(
                    "{:x}",
                    sha2::Sha256::digest(serde_json::to_vec(body).unwrap())
                )
            );
        }
        assert_eq!(digests.len(), bodies.len());
        report.push(json!({"case_id":if mode == 2 {"W12-fallback-expired"} else if expired {"W12-agent-expired"}else{"W11-agent-initial-followup"},"pass":true,"request_count":if mode == 2 {3}else{2},"world_sent_per_request":if mode == 2 {vec![true,false,false]}else{selected},"manifest_match":true}));
    }
    println!(
        "WORLD_EVAL_REPORT={}",
        json!({"schema_version":1,"suite":"world-m4a-agent","cases":report})
    );
}
