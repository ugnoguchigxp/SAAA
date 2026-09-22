#[tokio::test]
async fn g19_old_execution_ref_is_stale_after_a_revision_change() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    let minutes = response
        .candidates
        .iter()
        .find(|candidate| candidate.tool_id == "minutes")
        .expect("minutes candidate");
    let described = service
        .describe(&context, &minutes.reference, "contract", None)
        .expect("describe minutes");
    let minutes_execution = described.execution_ref.expect("execution ref");

    // Publish a new revision for the same tool after the reference was issued.
    let entry = CatalogEntry {
        tool_id: "minutes".to_string(),
        backend_key: "minutes".to_string(),
        title: "minutes".to_string(),
        purpose: "Updated minutes tool.".to_string(),
        operations: vec!["search".into()],
        objects: vec!["decision_record".into()],
        suitable: vec![],
        unsuitable: vec![],
        required_inputs: vec!["q".into()],
        input_schema: json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "required": ["q"],
            "additionalProperties": false
        }),
        output_schema: None,
        effect: "read",
        usage_pages: vec![],
        backend_binding: json!({"capabilityId":"minutes","revisionId":"minutes-rev2"}),
    };
    let now = now_ms();
    write_tx(&harness.writer, |connection| {
        catalog::register_revision(connection, PRINCIPAL, "llang", &entry, "minutes-rev2", now)
            .map_err(|error| error.code.as_str().to_string())
    })
    .expect("register revision two");

    let cancellation = crate::RunCancellation::default();
    let error = service
        .invoke(
            &context,
            &minutes_execution,
            &json!({"q":"v"}),
            &cancellation,
        )
        .await
        .expect_err("stale");
    assert_eq!(error.code, ToolSelectionErrorCode::StaleReference);
    assert_eq!(harness.backend.call_count(), 0);
}
#[tokio::test]
async fn g20_unavailable_reranker_degrades_without_faking_confidence() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service =
        harness.service_with_reranker(Arc::new(super::inference::UnavailableReranker), NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(response.status, DecisionStatus::Degraded);
    assert!(response.degraded);
    assert!(response.notes.iter().any(|note| note.contains("Reranking")));
}
#[tokio::test]
async fn describe_usage_pages_are_paginated_and_bound() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    let candidate = &response.candidates[0];
    let usage = service
        .describe(&context, &candidate.reference, "usage", None)
        .expect("usage");
    assert!(usage.body.get("text").is_some());
    assert!(usage.execution_ref.is_some());
}
#[tokio::test]
async fn search_response_contract_is_bounded() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    for candidate in &response.candidates {
        assert!(candidate.summary.len() <= super::contracts::SEARCH_CANDIDATE_SUMMARY_MAX_BYTES);
    }
    assert!(response.candidates.len() <= 8);
}
#[tokio::test]
async fn gateway_search_describe_invoke_round_trip() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let cancellation = crate::RunCancellation::default();

    let search = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_SEARCH,
        &json!({ "intent": USER_MESSAGE, "limit": 3 }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(search.pointer("/ok"), Some(&json!(true)));
    let candidate_ref = search
        .pointer("/data/candidates/0/candidateRef")
        .and_then(Value::as_str)
        .expect("candidate ref")
        .to_string();

    let describe = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_DESCRIBE,
        &json!({ "candidateRef": candidate_ref }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(describe.pointer("/ok"), Some(&json!(true)));
    let execution_ref = describe
        .pointer("/data/executionRef")
        .and_then(Value::as_str)
        .expect("execution ref")
        .to_string();
    assert!(describe.pointer("/data/body/inputSchema").is_some());

    let invoke = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_INVOKE,
        &json!({ "executionRef": execution_ref, "arguments": { "q": "v" } }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(invoke.pointer("/ok"), Some(&json!(true)));
    assert_eq!(invoke.pointer("/data/status"), Some(&json!("succeeded")));
}
#[tokio::test]
async fn gateway_rejects_unknown_arguments_without_leaking_details() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let cancellation = crate::RunCancellation::default();
    let response = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_SEARCH,
        &json!({ "intent": USER_MESSAGE, "profile": "forged" }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(response.pointer("/ok"), Some(&json!(false)));
    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!("invalid-input"))
    );
    assert_eq!(
        response.pointer("/error/message"),
        Some(&json!("The tool selection request is invalid."))
    );
}
// --- Provider-backed extraction (mock HTTP provider) -----------------------------------------

async fn mock_json_provider(body: String) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let endpoint = format!("http://{}/proxy/v1", listener.local_addr().expect("addr"));
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = vec![0_u8; 16 * 1024];
        let _ = socket.read(&mut buffer).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.expect("write");
    });
    (endpoint, task)
}
impl Harness {
    fn configure_mock_provider(&self, endpoint: &str) {
        let endpoint = endpoint.to_string();
        write_tx(&self.writer, move |connection| {
            let mut providers = crate::persistence::load_model_providers(connection)?;
            providers
                .providers
                .push(crate::ModelProviderSettings::OpenAiCompatible(
                    crate::OpenAiCompatibleProviderSettings {
                        request_options: None,
                        id: "mock-extraction-provider".to_string(),
                        enabled: true,
                        label: "Mock extraction".to_string(),
                        location: "local".to_string(),
                        endpoint,
                        model: "fixture".to_string(),
                        authentication: "none".to_string(),
                    },
                ));
            let providers_json =
                serde_json::to_string(&providers).map_err(|error| error.to_string())?;
            connection
                .execute(
                    "UPDATE settings_documents SET value_json = ?1
                      WHERE namespace = 'providers.model' AND key = 'default'",
                    rusqlite::params![providers_json],
                )
                .map_err(|error| error.to_string())?;

            let mut routing = crate::persistence::load_routing_settings(connection)?;
            routing.conversation_respond.source = "provider".to_string();
            routing.conversation_respond.primary_provider_id =
                Some("mock-extraction-provider".to_string());
            let routing_json =
                serde_json::to_string(&routing).map_err(|error| error.to_string())?;
            connection
                .execute(
                    "UPDATE settings_documents SET value_json = ?1
                      WHERE namespace = 'routing.tasks' AND key = 'default'",
                    rusqlite::params![routing_json],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("configure mock provider");
    }
}
#[tokio::test]
async fn provider_extraction_uses_the_configured_conversation_provider() {
    let harness = Harness::new();
    harness.register_pair();
    let extraction = json!({
        "scenario": {
            "intent": "案件の過去の判断を確認",
            "operation": "search",
            "objectType": "decision_record",
            "phase": "discover",
            "inputKind": "text"
        },
        "feedback": [{
            "kind": "tool_choice",
            "decisionId": null,
            "rejectedToolId": "web",
            "preferredToolId": "minutes",
            "scope": "project",
            "duration": "persistent",
            "evidence": { "start": 0, "end": 12, "text": "この案件" },
            "condition": {
                "operation": "search",
                "objectType": "decision_record",
                "phase": null,
                "inputKind": null
            }
        }]
    })
    .to_string();
    let chat = json!({
        "model": "fixture",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": extraction },
            "finish_reason": "stop"
        }]
    })
    .to_string();
    let (endpoint, task) = mock_json_provider(chat).await;
    harness.configure_mock_provider(&endpoint);

    let extractor =
        super::provider_extraction::ConversationProviderExtractor::new(harness.writer.clone());
    let request = super::extraction::ExtractionRequest {
        user_message: USER_MESSAGE.to_string(),
        recent_decisions: Vec::new(),
        allowed_decisions: std::collections::HashSet::new(),
        allowed_tools: ["web".to_string(), "minutes".to_string()]
            .into_iter()
            .collect(),
        prompt_tools: vec!["web".to_string(), "minutes".to_string()],
    };
    let parsed = extractor.extract(request).await.expect("extract");
    task.await.expect("mock provider");
    assert_eq!(parsed.accepted.len(), 1);
    assert_eq!(parsed.scenario.object_type, ObjectType::DecisionRecord);
    assert_eq!(
        parsed.accepted[0].preferred_tool_id.as_deref(),
        Some("minutes")
    );
}
#[tokio::test]
async fn provider_extraction_absent_provider_degrades() {
    let harness = Harness::new();
    harness.register_pair();
    let extractor =
        super::provider_extraction::ConversationProviderExtractor::new(harness.writer.clone());
    let request = super::extraction::ExtractionRequest {
        user_message: USER_MESSAGE.to_string(),
        recent_decisions: Vec::new(),
        allowed_decisions: std::collections::HashSet::new(),
        allowed_tools: std::collections::HashSet::new(),
        prompt_tools: Vec::new(),
    };
    assert!(extractor.extract(request).await.is_err());
}
// --- E01: real L-Lang invoke through the selection backend ------------------------------------

impl Harness {
    fn with_writer(writer: Arc<SqliteWriter>) -> Self {
        Self {
            writer,
            backend: Arc::new(FixtureBackend::new()),
        }
    }

    fn service_with_backend(
        &self,
        backend: Arc<dyn super::backends::ToolBackend>,
        extractor_body: &str,
    ) -> ToolSelectionService {
        ToolSelectionService::new(
            self.writer.clone(),
            Arc::new(ConstantEmbedding),
            Arc::new(FixedReranker::new(&[("llang-fixture-rev1", 1.0)])),
            Arc::new(FixtureExtractor::new(extractor_body)),
            backend,
            f64::NEG_INFINITY,
        )
    }

    fn register_llang(
        &self,
        resolved: &crate::generated_capabilities::contracts::ResolvedCapability,
    ) {
        let input_fields: Vec<String> = resolved
            .contract
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect();
        let mut properties = serde_json::Map::new();
        for field in &input_fields {
            properties.insert(field.clone(), json!({ "type": "boolean" }));
        }
        let input_schema = json!({
            "type": "object",
            "properties": properties,
            "required": input_fields,
            "additionalProperties": false
        });
        let entry = CatalogEntry {
            tool_id: "llang-fixture".to_string(),
            backend_key: "llang-fixture".to_string(),
            title: "llang fixture".to_string(),
            purpose: "Verified L-Lang capability used by the deterministic fixtures.".to_string(),
            operations: vec!["execute".to_string()],
            objects: vec!["code".to_string()],
            suitable: vec!["boolean checks".to_string()],
            unsuitable: vec!["free text".to_string()],
            required_inputs: input_fields.clone(),
            input_schema,
            output_schema: None,
            effect: "pure",
            usage_pages: vec![UsagePage {
                section: "usage",
                page: 0,
                text: "Pass every boolean field.".to_string(),
            }],
            backend_binding: json!({
                "capabilityId": resolved.capability_id,
                "revisionId": resolved.revision_id,
                "packageHash": resolved.package_hash,
                "contractHash": resolved.contract_hash,
                "catalogEpoch": resolved.catalog_epoch,
                "inputFields": input_fields,
            }),
        };
        let revision_id = "llang-fixture-rev1".to_string();
        let now = now_ms();
        write_tx(&self.writer, move |connection| {
            catalog::register_revision(connection, PRINCIPAL, "llang", &entry, &revision_id, now)
                .map_err(|error| error.code.as_str().to_string())?;
            repository::upsert_embedding(connection, &revision_id, MODEL_HASH, &vector(1.0))
                .map_err(|error| error.to_string())?;
            repository::upsert_grant(connection, PRINCIPAL, "llang-fixture", "user", PRINCIPAL)
                .map_err(|error| error.to_string())?;
            repository::bump_epochs(connection, false, true, false)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("register llang tool");
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e01_real_llang_invoke_true_and_false_through_selection() {
    use crate::generated_capabilities::tests::{TestEnv, ACCEPTANCE_A, CANDIDATE_A};
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("resolve active");
    let fields: Vec<String> = resolved
        .contract
        .fields
        .iter()
        .map(|field| field.name.clone())
        .collect();
    let harness = Harness::with_writer(env.writer.clone());
    harness.register_llang(&resolved);

    let backend = Arc::new(super::backends::llang::LlangBackend::new(Some(
        env.service.clone(),
    )));
    let service = harness.service_with_backend(backend, NO_FEEDBACK);
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let response = service
        .search(&context, "verified boolean capability", 8)
        .await
        .expect("search");
    let candidate = response.candidates.first().expect("candidate");
    assert_eq!(candidate.tool_id, "llang-fixture");
    let described = service
        .describe(&context, &candidate.reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();

    for expected in [true, false] {
        // Fixture A is an enabled/suspended truth table: enabled=true, suspended=false is the
        // only true case; every other assignment is false.
        let mut arguments = serde_json::Map::new();
        for field in &fields {
            let value = match field.as_str() {
                "enabled" => expected,
                _ => false,
            };
            arguments.insert(field.clone(), Value::Bool(value));
        }
        let invoked = service
            .invoke(
                &context,
                &execution_ref,
                &Value::Object(arguments),
                &cancellation,
            )
            .await
            .expect("invoke");
        assert_eq!(invoked.status, super::backends::TechnicalStatus::Succeeded);
        let value = invoked
            .result
            .as_ref()
            .and_then(|result| result.get("value"))
            .and_then(Value::as_bool);
        assert_eq!(value, Some(expected));
    }
    assert_eq!(env.call_count(&revision.revision_id), 2);
}
