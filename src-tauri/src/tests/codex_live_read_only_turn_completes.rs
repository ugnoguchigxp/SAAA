use super::*;
#[cfg(not(coverage))]
#[test]
#[ignore = "requires a local Codex runtime, authentication, and network access"]
pub(super) fn codex_live_read_only_turn_completes() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = run_codex_turn_process(
        "run-live-smoke",
        "Reply with exactly SAAA_LIVE_OK. Do not use tools.",
        workspace.path(),
        "",
        None,
        120_000,
        &events,
        &RunCancellation::default(),
    )
    .expect("live Codex turn succeeds");
    assert!(outcome.content.contains("SAAA_LIVE_OK"));
    assert_eq!(
        fs::read_dir(workspace.path())
            .expect("workspace remains readable")
            .count(),
        0,
        "read-only Codex turn must not create workspace files"
    );
}
#[cfg(not(coverage))]
#[test]
#[ignore = "requires a local Codex runtime, authentication, and network access"]
pub(super) fn codex_live_read_only_turn_cancels_after_turn_start() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let cancellation = Arc::new(RunCancellation::default());
    let cancellation_for_events = cancellation.clone();
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |_| {
        cancellation_for_events.cancel();
        Ok(())
    });
    let failure = run_codex_turn_process(
        "run-live-cancel",
        "Explain the read-only runtime lifecycle in detail. Do not use tools.",
        workspace.path(),
        "",
        None,
        120_000,
        &events,
        &cancellation,
    )
    .expect_err("live Codex turn is cancelled after turn/start");
    assert_eq!(
        failure.code,
        runtime::contracts::RunFailureCode::UserCancelled
    );
    assert_eq!(
        fs::read_dir(workspace.path())
            .expect("workspace remains readable")
            .count(),
        0,
        "cancelled read-only Codex turn must not create workspace files"
    );
}
pub(super) fn world_body_history(conversation_id: &str) -> Vec<ConversationMessage> {
    vec![
        ConversationMessage {
            parts: None,
            id: "context-system".into(),
            conversation_id: conversation_id.into(),
            role: "system".into(),
            content: "policy".into(),
            created_at: "system".into(),
        },
        ConversationMessage {
            parts: None,
            id: "context-world".into(),
            conversation_id: conversation_id.into(),
            role: "assistant".into(),
            content: "WORLD_BLOCK_PRESENT".into(),
            created_at: "1".into(),
        },
        ConversationMessage {
            parts: None,
            id: "context-current".into(),
            conversation_id: conversation_id.into(),
            role: "user".into(),
            content: "hello".into(),
            created_at: "2".into(),
        },
    ]
}
pub(super) async fn run_world_body_case(
    valid: bool,
    run_id: &str,
    session_provider_id: &str,
) -> (Value, i64) {
    let world = crate::runtime::context::world::turn::WorldLive::for_test(
        valid,
        "WORLD_BLOCK_PRESENT",
        Some("WORLD_BLOCK_ABSENT"),
    );
    let history = world_body_history(PRIMARY_CONVERSATION_ID);
    let (sent, include_world) = world.provider_history(&history);
    let candidate = crate::runtime::context::source::Candidate::untrusted(
        format!("{run_id}-{session_provider_id}"),
        crate::runtime::context::world::source::WORLD_KIND,
        vec![],
        crate::runtime::context::source::Requirement::May,
        "world-source".into(),
        1,
        0,
        "world".into(),
    );
    let selected = [candidate];
    let (kept, omitted) =
        crate::runtime::context::world::dispatch::for_record(&selected, &[], include_world);
    assert_eq!(kept.len() + omitted.len(), 1);
    let body = json!({"messages":sent.iter().map(|m| json!({"role":m.role,"content":m.content})).collect::<Vec<_>>()});
    (body, kept.len() as i64)
}

#[tokio::test]
pub(super) async fn expired_world_block_is_removed_from_provider_history_and_record_selection() {
    let (body, selected_world) =
        run_world_body_case(false, "run-world-expired", "world-expired-fixture").await;
    let rendered = body["messages"].to_string();
    assert!(
        !rendered.contains("WORLD_BLOCK_PRESENT"),
        "an expired World block must not reach the provider body"
    );
    assert!(
        rendered.contains("WORLD_BLOCK_ABSENT"),
        "the World-free rendering must reach the provider body"
    );
    assert_eq!(
        selected_world, 0,
        "an expired World must not be recorded as selected"
    );
}
#[tokio::test]
pub(super) async fn valid_world_block_is_kept_in_provider_history_and_record_selection() {
    let (body, selected_world) =
        run_world_body_case(true, "run-world-valid", "world-valid-fixture").await;
    let rendered = body["messages"].to_string();
    assert!(
        rendered.contains("WORLD_BLOCK_PRESENT"),
        "a current World block must reach the provider body"
    );
    assert!(
        !rendered.contains("WORLD_BLOCK_ABSENT"),
        "the World-free rendering must not replace a current World block"
    );
    assert_eq!(
        selected_world, 1,
        "a current World must be recorded as selected"
    );
}
pub(crate) fn two_step_role_policy(front_provider: &str, reason_provider: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"front","label":"Front","aliases":[],"transport":"provider",
            "providerId":front_provider,"model":null,"location":"local",
            "resourceGroup":"front","maxInputBytes":16384,"capabilities":["reason"]
        }, {
            "id":"reason","label":"Reason","aliases":[],"transport":"provider",
            "providerId":reason_provider,"model":null,"location":"local",
            "resourceGroup":"reason","maxInputBytes":16384,"capabilities":["reason"]
        }],
        "roles":{"frontend":"front","reasoner":"reason","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
        "recipes":[{"id":"ack-then-reason","action":"respond","roles":["frontend","reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":2,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
pub(super) fn reviewed_role_policy(author_provider: &str, reviewer_provider: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"author","label":"Author","aliases":[],"transport":"provider",
            "providerId":author_provider,"model":null,"location":"local",
            "resourceGroup":"author","maxInputBytes":65536,"capabilities":["reason"]
        }, {
            "id":"reviewer","label":"Reviewer","aliases":[],"transport":"provider",
            "providerId":reviewer_provider,"model":null,"location":"local",
            "resourceGroup":"reviewer","maxInputBytes":65536,"capabilities":["reason"]
        }],
        "roles":{"frontend":null,"reasoner":"author","advanced":null,"reviewer":"reviewer","premium":null,"toolSpecialist":null},
        "recipes":[{"id":"reviewed-response","action":"respond","roles":["reasoner","reviewer","reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":3,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
pub(crate) fn specialist_role_policy(author_provider: &str, specialist_provider: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"author","label":"Author","aliases":[],"transport":"provider",
            "providerId":author_provider,"model":null,"location":"local",
            "resourceGroup":"author","maxInputBytes":65536,"capabilities":["reason"]
        }, {
            "id":"specialist","label":"Specialist","aliases":[],"transport":"provider",
            "providerId":specialist_provider,"model":null,"location":"local",
            "resourceGroup":"specialist","maxInputBytes":65536,"capabilities":["reason"]
        }],
        "roles":{"frontend":null,"reasoner":"author","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":"specialist"},
        "recipes":[{"id":"specialist-response","action":"respond","roles":["reasoner","tool_specialist","reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":3,"maxToolCalls":1,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
pub(crate) fn single_role_policy(provider_id: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"author","label":"Author","aliases":[],"transport":"provider",
            "providerId":provider_id,"model":null,"location":"local",
            "resourceGroup":"author","maxInputBytes":16384,"capabilities":["reason"]
        }],
        "roles":{"frontend":null,"reasoner":"author","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
        "recipes":[{"id":"direct-response","action":"respond","roles":["reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":1,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
#[test]
pub(super) fn rr_25_offline_review_policy_retains_author_and_reviewer_contracts() {
    let policy = reviewed_role_policy("author-provider", "reviewer-provider");
    let encoded = policy.to_string();
    assert!(encoded.contains("author-provider"));
    assert!(encoded.contains("reviewer-provider"));
    assert!(encoded.contains("review"));
}
