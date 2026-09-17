use super::*;
#[tokio::test]
async fn coding_sse_bridge_roundtrips_host_errors_without_simulated_success() {
    coding_roundtrip(false).await;
    coding_roundtrip(true).await;
}
async fn coding_roundtrip(accepted: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for round in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (_, body) = request(&mut socket).await;
            let input: Value =
                serde_json::from_str(body["input"][0]["text"].as_str().unwrap()).unwrap();
            let output = if round == 0 {
                assert_eq!(input["codingTools"].as_array().unwrap().len(), 4);
                let instructions = input["instructions"].as_str().unwrap();
                let start = instructions.find("<saaa-coding-").unwrap();
                let end = start + instructions[start..].find('>').unwrap() + 1;
                format!(
                    "{}{}{}",
                    &instructions[start..end],
                    json!({"name":"coding_start","arguments":{"workspaceId":"invented","request":"implement"}}),
                    "</saaa-coding>"
                )
            } else if accepted {
                assert_eq!(
                    input["toolResult"]["input"]["result"]["result"]["accepted"],
                    true
                );
                String::new()
            } else {
                assert_eq!(
                    input["toolResult"]["input"]["result"]["result"]["error"]["code"],
                    "workspace_required"
                );
                "作業フォルダーを選択してください。まだ開始していません。".into()
            };
            respond(
                &mut socket,
                "application/json",
                &json!({"id":format!("agt_{round}"),"session_id":"ags_coding"}).to_string(),
            )
            .await;
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let mut events = String::new();
            for (i, ch) in output.chars().enumerate() {
                events += &format!(
                    "id: {round}-{i}\ndata: {}\n\n",
                    json!({"type":"message.delta","session_id":"ags_coding","turn_id":format!("agt_{round}"),"data":{"text":ch.to_string()}})
                );
            }
            events += &format!(
                "id: end-{round}\ndata: {}\n\n",
                json!({"type":if accepted && round==1{"turn.failed"}else{"turn.completed"},"session_id":"ags_coding","turn_id":format!("agt_{round}"),"data":{}})
            );
            respond(&mut socket, "text/event-stream", &events).await;
        }
    });
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute(
        "UPDATE coding_settings SET value_json=json_set(value_json,'$.enabled',json('true'))",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO conversation_messages VALUES('coding_input',?1,'user','implement','1')",
        [crate::PRIMARY_CONVERSATION_ID],
    )
    .unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('coding_sse',?1,'conversation.respond','running','coding_input','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    if accepted {
        use sha2::{Digest, Sha256};
        let arguments = json!({"workspaceId":"invented","request":"implement"});
        let digest = format!("{:x}", Sha256::digest(format!("coding_start:{arguments}")));
        c.execute("INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id) VALUES('job',?1,'coding_input','invented','/tmp','{}',1,'/tmp/fixture-session','settled','run')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
        c.execute("INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES('run','job','coding_input','coding_sse','implement',?1,'accepted','settled','1')",[digest]).unwrap();
    }
    let state = crate::test_support::app_state(c);
    let persistence_id = crate::begin_provider_session(
        &state,
        "coding_sse",
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    let input = crate::StartTurnInput {
        run_id: "coding_sse".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: "implement".into(),
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
        model: "fixture".into(),
        models_path: "/models".into(),
        sessions_path: "/v1/agents/sessions".into(),
        authentication: "none".into(),
    };
    let session = SessionResponse {
        id: "ags_coding".into(),
        events_url: None,
    };
    let history = [ConversationMessage {
        parts: None,
        id: "coding_input".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: "1".into(),
    }];
    let outcome = run_agent_session_sse(
        &Client::new(),
        &provider,
        &session,
        Url::parse(&format!("{base}/events")).unwrap(),
        &history,
        10000,
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
    if accepted {
        assert!(
            matches!(
                outcome,
                ProviderAttemptOutcome::Failed {
                    output_started: true,
                    ..
                }
            ),
            "{outcome:?}"
        );
    } else {
        assert!(
            matches!(outcome, ProviderAttemptOutcome::Completed { .. }),
            "{outcome:?}"
        );
    }
    server.await.unwrap();
    let text = sink
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| {
            if let RuntimeEvent::Delta { text, .. } = event {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<String>();
    assert_eq!(
        text,
        if accepted {
            ""
        } else {
            "作業フォルダーを選択してください。まだ開始していません。"
        }
    );
}
