#[tokio::test]
async fn revoke_of_an_unresolvable_tool_keeps_existing_rules() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message.clone()), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    assert!(harness.count("tool_selection_rules") > 0);

    // A revoke that names a tool the host cannot resolve must not wipe every correction in scope.
    let parsed = ParsedExtraction {
        scenario: Scenario::degraded("撤回"),
        accepted: vec![ExtractedFeedback {
            kind: FeedbackKind::Revoke,
            decision_id: None,
            rejected_tool_id: Some("ghost-tool".into()),
            preferred_tool_id: None,
            scope: ScopeKind::Project,
            duration: Duration::Persistent,
            evidence: Evidence {
                start: 0,
                end: 6,
                text: "この案件".into(),
            },
            condition: FeedbackCondition::default(),
        }],
        rejected: Vec::new(),
    };
    let outcome = service.apply_parsed(&context, &parsed).expect("apply");
    assert!(outcome.ambiguous);
    let active: i64 = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tool_selection_rules WHERE state = 'active'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("count");
    assert_eq!(active, 2);
}
#[tokio::test]
async fn startup_reconcile_settles_running_invocations() {
    let harness = Harness::new();
    harness.register_pair();
    write_tx(&harness.writer, |connection| {
        connection
            .execute(
                "INSERT INTO tool_selection_invocations(
                   id, decision_id, revision_id, technical_status, satisfaction, started_at)
                 VALUES ('stale-invocation', NULL, 'web-rev1', 'running', 'unknown', 1)",
                [],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    })
    .expect("insert running invocation");
    let repaired =
        super::service::reconcile_interrupted_invocations(&harness.writer).expect("reconcile");
    assert_eq!(repaired, 1);
    let status: String = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT technical_status FROM tool_selection_invocations
                      WHERE id = 'stale-invocation'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("status");
    assert_eq!(status, "interrupted");
}
#[tokio::test]
async fn explicit_preference_injects_a_tool_retrieval_missed() {
    let harness = Harness::new();
    // 35 noise tools dominate the vector/lexical pools so `minutes` is outside the fused top 30.
    for index in 0..35 {
        let mut noise = ToolSpec::tool(
            Box::leak(format!("noise_{index:02}").into_boxed_str()),
            vector(0.9),
            None,
        );
        noise.purpose = "Unrelated fixture utility.";
        harness.register(&noise);
    }
    let mut minutes = ToolSpec::tool("minutes", vector(0.0), None);
    minutes.purpose = "Internal meeting minutes for decision records.";
    harness.register(&minutes);

    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
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
            "rejectedToolId": null,
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
    let service = harness.service_with_reranker(
        Arc::new(FixedReranker::new(&[("minutes-rev1", 5.0)])),
        &body,
    );
    let turn = service.begin_turn(&context, USER_MESSAGE).await;
    assert!(turn
        .apply
        .as_ref()
        .map(|apply| apply.applied)
        .unwrap_or(false));
    let response = service
        .search(&context, "unrelated query text", 8)
        .await
        .expect("search");
    assert_eq!(
        response.candidates.first().map(|c| c.tool_id.as_str()),
        Some("minutes")
    );
}
#[tokio::test]
async fn missing_embedding_index_is_reported_as_degraded_not_no_match() {
    let harness = Harness::new();
    harness.register_pair();
    harness
        .writer
        .write(|connection| {
            connection
                .execute("DELETE FROM tool_selection_embeddings", [])
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("clear embeddings");
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(response.status, DecisionStatus::Degraded);
    assert!(response.degraded);
}
#[tokio::test]
async fn gateway_rejects_wrong_typed_describe_fields() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let cancellation = crate::RunCancellation::default();
    let response = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_DESCRIBE,
        &json!({ "candidateRef": "whatever", "section": 5 }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(response.pointer("/ok"), Some(&json!(false)));
    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!("invalid-input"))
    );
}
/// Backend that cycles success/failure/unknown so a batch has mixed technical outcomes.
struct MixedBackend {
    counter: AtomicUsize,
    calls: Mutex<Vec<BackendRequest>>,
}
impl MixedBackend {
    fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
        }
    }
}
#[async_trait]
impl ToolBackend for MixedBackend {
    async fn invoke(
        &self,
        request: BackendRequest,
        cancellation: &crate::RunCancellation,
    ) -> BackendOutcome {
        if cancellation.is_cancelled() {
            return BackendOutcome::cancelled();
        }
        self.calls.lock().expect("calls").push(request);
        match self.counter.fetch_add(1, Ordering::SeqCst) % 3 {
            0 => BackendOutcome::succeeded(json!({ "ok": true })),
            1 => BackendOutcome::failed("remote-failure"),
            _ => BackendOutcome {
                status: super::backends::TechnicalStatus::Unknown,
                result: None,
                error_code: Some("remote-outcome-unknown"),
            },
        }
    }
}
#[tokio::test]
async fn p03_100_mixed_calls_never_invent_positive_satisfaction() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let backend = Arc::new(MixedBackend::new());
    let service = ToolSelectionService::new(
        harness.writer.clone(),
        Arc::new(ConstantEmbedding),
        Arc::new(FixedReranker::new(&[
            ("web-rev1", 2.0),
            ("minutes-rev1", 1.0),
        ])),
        Arc::new(FixtureExtractor::new(NO_FEEDBACK)),
        backend.clone(),
        f64::NEG_INFINITY,
    );
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    let candidate = response.candidates.first().expect("candidate");
    let described = service
        .describe(&context, &candidate.reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();
    for _ in 0..100 {
        let _ = service
            .invoke(
                &context,
                &execution_ref,
                &json!({ "q": "v" }),
                &cancellation,
            )
            .await;
    }
    assert_eq!(backend.calls.lock().expect("calls").len(), 100);
    let (recorded, non_unknown): (i64, i64) = harness
        .writer
        .read_serialized(|connection| {
            let recorded = connection
                .query_row("SELECT COUNT(*) FROM tool_selection_invocations", [], |row| {
                    row.get(0)
                })
                .map_err(|error| error.to_string())?;
            let non_unknown = connection
                .query_row(
                    "SELECT COUNT(*) FROM tool_selection_invocations WHERE satisfaction <> 'unknown'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            Ok((recorded, non_unknown))
        })
        .expect("counts");
    assert_eq!(recorded, 100);
    assert_eq!(
        non_unknown, 0,
        "mixed success/failure/unknown calls must never invent satisfaction"
    );
}
