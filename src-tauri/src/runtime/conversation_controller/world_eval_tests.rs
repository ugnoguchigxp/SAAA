#![cfg(test)]
use super::*;
use crate::memory::personal_state::world::runtime_test_support::RUN_ID;
use crate::runtime::context::world::{g1_tests as graph, turn::compose_parts};
use crate::runtime::context::world::wire_test_support::{Harness, TRANSITIONS};
use sha2::{Digest, Sha256};
use serde_json::Value;

#[tokio::test]
async fn world_m4a_reasoning_wire() {
    reasoning_wire(false).await;
}
#[tokio::test]
async fn wr_t16_mcp_refreshes_after_initialization_and_records_exact_frame() {
    reasoning_wire(true).await;
}
async fn reasoning_wire(fresh_sources: bool) {
    let mut results = Vec::new();
    for mode in ["current", "expired", "slow-init"] {
        let expired = mode != "current" && !fresh_sources;
        let f = graph::g1_fixture();
        let scope = graph::load_scope(&f);
        let access = f.access();
        let composed = compose_parts(
            true,
            Some(Arc::new(if fresh_sources {
                f.service().with_sources(Arc::new(
                    crate::situation::SituationRuntime::new(Default::default(), None).unwrap(),
                ))
            } else {
                f.service()
            })),
            access.principal,
            access.policy_revision,
            Some(graph::graph_request("tech")),
            RUN_ID,
            &scope,
            graph::window(),
            vec![],
            graph::allowed(&scope),
        )
        .unwrap();
        let capabilities = Arc::new(
            crate::generated_capabilities::service::CapabilityService::build(
                f.writer.clone(),
                &std::path::PathBuf::new(),
                std::path::PathBuf::new(),
                None,
            ),
        );
        let state = crate::test_state::app_state_with_capabilities(f.writer.clone(), capabilities);
        let input: StartTurnInput = serde_json::from_value(serde_json::json!({"runId":RUN_ID,"conversationId":crate::PRIMARY_CONVERSATION_ID,"content":"hello","inputOrigin":"voice","presentationMode":"visual"})).unwrap();
        let history = composed
            .envelope
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| ConversationMessage {
                id: format!("context-{i}"),
                conversation_id: input.conversation_id.clone(),
                role: m.role.clone(),
                content: m.content.clone(),
                created_at: "now".into(),
                parts: None,
            })
            .collect::<Vec<_>>();
        if mode == "expired" {
            f.set_now(f.now() + 1000);
        }
        let server = crate::providers::reasoning_mcp::tests::fixture(mode).await;
        let client = crate::providers::reasoning_mcp::Client::new(
            &server.url,
            "fixture-token-long-enough".into(),
        )
        .unwrap();
        let sink = tauri::ipc::Channel::<RuntimeEvent>::new(|_| Ok(()));
        let response = execute(
            &state,
            &input,
            &history,
            &sink,
            Arc::default(),
            &client,
            ContextManifest {
                selected: &composed.envelope.selected,
                omitted: &composed.envelope.omitted,
                health: "green",
                world: composed.world.as_ref(),
            },
        );
        if mode == "slow-init" {
            let (response, ()) = tokio::join!(response, async {
                server.entered.notified().await;
                f.set_now(f.now() + 1000);
            });
            response.unwrap();
        } else {
            response.await.unwrap();
        }
        let requests = server.calls.lock().unwrap();
        let calls = requests
            .iter()
            .filter(|r| r["method"] == "tools/call")
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1);
        let body = &calls[0]["params"]["arguments"];
        let sent = body["context"]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"].as_str().unwrap().starts_with("world-model:"));
        assert_eq!(sent, !expired);
        assert!(!body["context"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"].as_str().unwrap().contains("[WORLD_MODEL")));
        let recorded: bool = f.writer.read_serialized(|c| c.query_row("SELECT EXISTS(SELECT 1 FROM context_generation_inputs WHERE source_kind='world-model' AND selected=1)", [], |r| r.get(0)).map_err(crate::database_error)).unwrap();
        assert_eq!(recorded, sent);
        let digest: String = f
            .writer
            .read_serialized(|c| {
                c.query_row(
                    "SELECT request_digest FROM context_generations ORDER BY ordinal DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })
            .unwrap();
        use sha2::Digest;
        assert_eq!(
            digest,
            format!(
                "{:x}",
                sha2::Sha256::digest(serde_json::to_vec(body).unwrap())
            )
        );
        results.push(serde_json::json!({"case_id":if mode == "slow-init" {"WD-MCP-connect-expired"} else if expired {"WD-MCP-expired"}else{"WD-MCP-current"},"pass":true,"request_count":1,"world_sent_per_request":[sent],"manifest_match":true}));
    }
    println!(
        "WORLD_EVAL_REPORT={}",
        serde_json::json!({"schema_version":1,"suite":"world-m4a-reasoning","cases":results})
    );
}

#[test]
fn world_g1_reasoning_receipt_excludes_world_when_required_evidence_fills_slots() {
    use crate::runtime::context::source::{Candidate, Requirement};
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::initialize_database(&connection).unwrap();
    let state = crate::test_support::app_state(connection);
    let input: StartTurnInput = serde_json::from_value(serde_json::json!({"runId":"run_test","conversationId":"conversation_test","content":"hello","inputOrigin":"voice","presentationMode":"visual"})).unwrap();
    let mut selected = (0..7)
        .map(|i| {
            Candidate::untrusted(
                format!("must-{i}"),
                "personal-state",
                vec![],
                Requirement::Must,
                format!("must-{i}"),
                1,
                1,
                format!("required-{i}"),
            )
        })
        .collect::<Vec<_>>();
    selected.push(Candidate::untrusted(
        "world".into(),
        "world-model",
        vec![],
        Requirement::May,
        "frame".into(),
        1,
        1,
        "world-body".into(),
    ));
    let world = crate::runtime::context::world::turn::WorldLive::for_test(true, "world-body", None);
    let (request, sent, omitted) = payload::prepare(
        &state,
        &input,
        &[],
        &ContextManifest {
            selected: &selected,
            omitted: &[],
            health: "green",
            world: Some(&world),
        },
    )
    .unwrap();
    assert_eq!(sent.len(), 7);
    assert!(sent.iter().all(|s| s.requirement == Requirement::Must));
    assert!(omitted.iter().any(|s| s.source_kind == "world-model"));
    assert!(!serde_json::to_string(&request)
        .unwrap()
        .contains("world-body"));
}

#[tokio::test]
async fn wr_t22_reasoning_mcp_route_transition_matrix() {
    for transition in TRANSITIONS {
        reasoning_matrix_case(transition).await;
    }
}

async fn reasoning_matrix_case(transition: &str) {
    if transition == "tool-continuation" {
        println!("WORLD_MATRIX_CASE={}", serde_json::json!({
            "case_id":"reasoning-mcp:tool-continuation", "route":"reasoning-mcp",
            "transition":transition, "source_kinds":[], "frame_digest":null,
            "wire_digest":null, "expected":"n/a-no-host-tool-capability",
            "actual":"n/a-no-host-tool-capability", "pass":true,
            "verification_level":"offline-contract", "omission_reason":"unsupported-capability",
            "request_count":0
        }));
        return;
    }

    let h = Harness::new();
    if matches!(transition, "correction" | "forget" | "scope-switch") {
        h.transition(transition);
    }
    if transition == "session-resume" {
        h.fixture.set_now(h.fixture.now() + 3_000);
    }
    let mut history = h.history.clone();
    let mut requests = Vec::new();
    if transition == "fallback" {
        let stale = crate::providers::reasoning_mcp::tests::fixture("stale").await;
        assert!(run_reasoning_matrix(&h, &history, &stale).await.is_err());
        requests.extend(tool_calls(&stale));
        let sent=requests.last().unwrap()["params"]["arguments"]["context"]["evidence"].as_array().unwrap().iter().find(|item|item["source"].as_str().is_some_and(|v|v.starts_with("world-model:"))).unwrap()["content"].as_str().unwrap().to_string();
        history.iter_mut().filter(|message| message.content.contains("[WORLD_MODEL")).for_each(|message| message.content=sent.clone());
    }
    let server = crate::providers::reasoning_mcp::tests::fixture("current").await;
    let result = run_reasoning_matrix(&h, &history, &server).await;
    let denied = transition == "scope-switch";
    assert_eq!(result.is_err(), denied, "reasoning-mcp/{transition}: {result:?}");
    requests.extend(tool_calls(&server));

    let body = requests.last().map(|call| call["params"]["arguments"].clone());
    let world = body.as_ref().and_then(|request| request["context"]["evidence"].as_array())
        .and_then(|items| items.iter().find(|item| item["source"].as_str().is_some_and(|v| v.starts_with("world-model:"))));
    if !denied {
        let evidence = world.expect("current World evidence");
        if matches!(transition, "correction" | "forget") {
            assert!(!evidence["content"].as_str().unwrap().contains("Speculative Decoding"));
        }
        let digest: String = h.fixture.writer.read_serialized(|c| c.query_row("SELECT request_digest FROM context_generations ORDER BY ordinal DESC LIMIT 1", [], |r|r.get(0)).map_err(crate::database_error)).unwrap();
        assert_eq!(digest, format!("{:x}", Sha256::digest(serde_json::to_vec(body.as_ref().unwrap()).unwrap())));
    } else {
        assert!(body.is_none());
    }
    let source_kinds = world.and_then(|e| e["world"]["sourceKinds"].as_array()).cloned().unwrap_or_default();
    println!("WORLD_MATRIX_CASE={}", serde_json::json!({
        "case_id":format!("reasoning-mcp:{transition}"), "route":"reasoning-mcp", "transition":transition,
        "source_kinds":source_kinds,
        "frame_digest":world.and_then(|e| e["content"].as_str()).map(|v|format!("{:x}",Sha256::digest(v.as_bytes()))),
        "wire_digest":body.as_ref().map(|v|format!("{:x}",Sha256::digest(serde_json::to_vec(v).unwrap()))),
        "expected":if denied{"denied-before-tools-call"}else{"current-frame-and-matching-receipt"},
        "actual":if denied{"denied-before-tools-call"}else{"current-frame-and-matching-receipt"},
        "pass":true,"verification_level":"offline-wire",
        "omission_reason":if denied{Some("scope-changed")}else{None},"request_count":requests.len()
    }));
}

async fn run_reasoning_matrix(
    h: &Harness,
    history: &[ConversationMessage],
    server: &crate::providers::reasoning_mcp::tests::Fixture,
) -> Result<ConversationMessage, String> {
    let client = crate::providers::reasoning_mcp::Client::new(&server.url, "fixture-token-long-enough".into()).unwrap();
    let input: StartTurnInput = serde_json::from_value(serde_json::json!({"runId":RUN_ID,"conversationId":crate::PRIMARY_CONVERSATION_ID,"content":"hello","inputOrigin":"voice","presentationMode":"visual"})).unwrap();
    let sink = tauri::ipc::Channel::<RuntimeEvent>::new(|_| Ok(()));
    execute(&h.state, &input, history, &sink, Arc::default(), &client, ContextManifest {
        selected:&h.composed.envelope.selected, omitted:&h.composed.envelope.omitted,
        health:"green", world:h.composed.world.as_ref(),
    }).await
}

fn tool_calls(server: &crate::providers::reasoning_mcp::tests::Fixture) -> Vec<Value> {
    server.calls.lock().unwrap().iter().filter(|call|call["method"]=="tools/call").cloned().collect()
}
