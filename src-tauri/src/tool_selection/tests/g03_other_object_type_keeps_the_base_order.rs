use super::super::backends::{BackendOutcome, BackendRequest, FixtureBackend, ToolBackend};
use super::super::catalog::{self, CatalogEntry, UsagePage};
use super::super::contracts::*;
use super::super::extraction::{CorrectionExtractor, FixtureExtractor};
use super::super::feedback::ParsedExtraction;
use super::super::inference::{
    EmbedKind, EmbeddingProvider, FixedReranker, InferenceError, RerankProvider,
};
use super::super::repository::{self, Epochs};
use super::super::service::ToolSelectionService;
use super::*;
use crate::persistence::SqliteWriter;
use async_trait::async_trait;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
#[tokio::test]
pub(super) async fn g03_other_object_type_keeps_the_base_order() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    service.set_scenario(
        &context,
        Scenario {
            intent: "競合の最新情報".into(),
            operation: Operation::Search,
            object_type: ObjectType::CurrentInformation,
            phase: Phase::Discover,
            input_kind: InputKind::Text,
        },
    );
    let response = service
        .search(&context, "競合の最新情報", 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
}
#[tokio::test]
pub(super) async fn g04_other_project_and_principal_are_not_corrected() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    harness.grant_user("P2", "web");
    harness.grant_user("P2", "minutes");
    harness.grant_user("P2", "archive");
    harness.insert_conversation("conversation_b");
    harness.insert_conversation("conversation_p2");
    let other_b = RequestContext::new(PRINCIPAL, "conversation_b")
        .with_run(Some("run-b".into()))
        .with_project(Some("B".to_string()));
    let other_p2 = RequestContext::new("P2", "conversation_p2")
        .with_run(Some("run-p2".into()))
        .with_project(Some("A".to_string()));
    for other in [other_b, other_p2] {
        service.set_scenario(
            &other,
            Scenario {
                intent: USER_MESSAGE.into(),
                operation: Operation::Search,
                object_type: ObjectType::DecisionRecord,
                phase: Phase::Discover,
                input_kind: InputKind::Text,
            },
        );
        let response = service
            .search(&other, USER_MESSAGE, 8)
            .await
            .expect("search");
        assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
    }
}
#[tokio::test]
pub(super) async fn g05_unknown_object_does_not_receive_the_correction() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    service.set_scenario(&context, Scenario::degraded(USER_MESSAGE));
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
}
#[tokio::test]
pub(super) async fn g06_repeated_message_is_idempotent() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let epoch_after_first = harness.epochs().rule;
    let second = service.begin_turn(&context, USER_MESSAGE).await;
    assert!(second
        .apply
        .as_ref()
        .map(|apply| apply.duplicate)
        .unwrap_or(false));
    assert_eq!(harness.epochs().rule, epoch_after_first);
    assert_eq!(harness.count("tool_selection_feedback"), 1);
    assert_eq!(harness.count("tool_selection_rules"), 2); // avoid web + prefer minutes
}
#[tokio::test]
pub(super) async fn g07_arguments_correction_creates_no_tool_choice_rule() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"フォルダが違う","operation":"unknown","objectType":"unknown","phase":"unknown","inputKind":"unknown"},
        "feedback": [{
            "kind": "arguments",
            "decisionId": null,
            "rejectedToolId": null,
            "preferredToolId": null,
            "scope": "conversation",
            "duration": "once",
            "evidence": {"start":0,"end":9,"text":"この案"},
            "condition": {"operation":null,"objectType":null,"phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    service
        .begin_turn(&context, "この案件のフォルダが違う")
        .await;
    assert_eq!(harness.count("tool_selection_rules"), 0);
    assert_eq!(harness.count("tool_selection_feedback"), 1);
}
#[tokio::test]
pub(super) async fn g08_ambiguous_correction_creates_no_rule() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"違う","operation":"unknown","objectType":"unknown","phase":"unknown","inputKind":"unknown"},
        "feedback": [{
            "kind": "ambiguous",
            "decisionId": null,
            "rejectedToolId": null,
            "preferredToolId": null,
            "scope": "conversation",
            "duration": "unspecified",
            "evidence": {"start":0,"end":6,"text":"違う"},
            "condition": {"operation":null,"objectType":null,"phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    let turn = service.begin_turn(&context, "違う").await;
    assert!(turn
        .apply
        .as_ref()
        .map(|apply| apply.ambiguous)
        .unwrap_or(false));
    assert_eq!(harness.count("tool_selection_rules"), 0);
}
#[tokio::test]
pub(super) async fn g09_successful_invocation_does_not_create_positive_feedback() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
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
    let invoked = service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "value" }),
            &cancellation,
        )
        .await
        .expect("invoke");
    assert_ne!(
        invoked.status,
        super::super::backends::TechnicalStatus::Cancelled
    );
    let satisfaction: String = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT satisfaction FROM tool_selection_invocations LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("satisfaction");
    assert_eq!(satisfaction, "unknown");
}
#[tokio::test]
pub(super) async fn p01_caller_abort_does_not_leave_the_invocation_running() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let backend: Arc<dyn ToolBackend> = Arc::new(BarrierBackend {
        entered: entered.clone(),
        release: release.clone(),
    });
    let service = Arc::new(harness.service_with_backend(backend, NO_FEEDBACK));
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

    let caller = {
        let service = service.clone();
        let context = context.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            service
                .invoke(
                    &context,
                    &execution_ref,
                    &json!({ "q": "value" }),
                    &cancellation,
                )
                .await
        })
    };
    let _entry_permit = tokio::time::timeout(std::time::Duration::from_secs(5), entered.acquire())
        .await
        .expect("backend entered within timeout")
        .expect("entry permit");
    // Aborting the caller drops only the response future; the management task must continue.
    caller.abort();
    let _ = caller.await;
    release.add_permits(1);

    let mut status = String::new();
    for _ in 0..400 {
        status = harness
            .writer
            .read_serialized(|connection| {
                connection
                    .query_row(
                        "SELECT technical_status FROM tool_selection_invocations LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|error| error.to_string())
            })
            .expect("invocation status");
        if status != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(status, "succeeded", "aborted caller left the row unsettled");
}
#[tokio::test]
pub(super) async fn g10_once_without_task_is_conversation_scoped() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "conversation", "once"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let same = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&same), vec!["minutes", "web", "archive"]);
    let other = RequestContext::new(PRINCIPAL, "conversation_other")
        .with_run(Some("run-other".into()))
        .with_project(Some("A".to_string()));
    harness.insert_conversation("conversation_other");
    service.set_scenario(
        &other,
        Scenario {
            intent: USER_MESSAGE.into(),
            operation: Operation::Search,
            object_type: ObjectType::DecisionRecord,
            phase: Phase::Discover,
            input_kind: InputKind::Text,
        },
    );
    let other = service
        .search(&other, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&other), vec!["web", "minutes", "archive"]);
}
#[tokio::test]
pub(super) async fn g11_revoke_removes_the_active_rule_and_bumps_epoch() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let before = harness.epochs().rule;
    let revoke = json!({
        "scenario": {"intent":"撤回","operation":"unknown","objectType":"unknown","phase":"unknown","inputKind":"unknown"},
        "feedback": [{
            "kind": "revoke",
            "decisionId": null,
            "rejectedToolId": "minutes",
            "preferredToolId": null,
            "scope": "project",
            "duration": "persistent",
            "evidence": {"start":0,"end":6,"text":"さっ"},
            "condition": {"operation":null,"objectType":null,"phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let revoke_service = harness.service(&revoke);
    revoke_service
        .begin_turn(&context, "さっきの指定は撤回")
        .await;
    assert!(harness.epochs().rule > before);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
    assert_eq!(harness.count("tool_selection_feedback"), 2);
}
#[tokio::test]
pub(super) async fn g12_unauthorized_tool_never_appears_even_with_a_preference() {
    let harness = Harness::new();
    let web = ToolSpec::tool("web", vector(1.0), Some("A"));
    harness.register(&web);
    // minutes belongs to project B only, so it is not authorized in project A.
    let minutes = ToolSpec::tool("minutes", vector(0.9), Some("B"));
    harness.register(&minutes);
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert!(order(&response).contains(&"web".to_string()));
    assert!(!order(&response).contains(&"minutes".to_string()));
}
#[tokio::test]
pub(super) async fn g13_rule_commit_during_inference_triggers_one_research() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let reranker = Arc::new(MutatingReranker::new(
        harness.writer.clone(),
        &[("web-rev1", 2.0), ("minutes-rev1", 1.0)],
    ));
    let service = harness.service_with_reranker(reranker.clone(), NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(reranker.calls.load(Ordering::SeqCst), 2);
    assert!(!response.decision_id.is_empty());
}
#[tokio::test]
pub(super) async fn g14_correction_after_describe_makes_invoke_change_selection() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    let candidate = response.candidates.first().expect("candidate");
    let described = service
        .describe(&context, &candidate.reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");

    let correction = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    correction.begin_turn(&context, USER_MESSAGE).await;

    let cancellation = crate::RunCancellation::default();
    let error = service
        .invoke(&context, &execution_ref, &json!({"q": "v"}), &cancellation)
        .await
        .expect_err("must change");
    assert_eq!(error.code, ToolSelectionErrorCode::SelectionChanged);
    assert_eq!(harness.backend.call_count(), 0);
}
#[tokio::test]
pub(super) async fn g15_unknown_decision_id_is_rejected() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"x","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},
        "feedback": [{
            "kind":"tool_choice","decisionId":"not-ours","rejectedToolId":"web","preferredToolId":"minutes",
            "scope":"project","duration":"persistent",
            "evidence":{"start":0,"end":12,"text":"この案件"},
            "condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    service.begin_turn(&context, USER_MESSAGE).await;
    assert_eq!(harness.count("tool_selection_rules"), 0);
    assert_eq!(harness.count("tool_selection_feedback"), 1);
}
#[tokio::test]
pub(super) async fn g16_invalid_byte_boundary_is_rejected() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"x","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},
        "feedback": [{
            "kind":"tool_choice","decisionId":null,"rejectedToolId":"web","preferredToolId":"minutes",
            "scope":"project","duration":"persistent",
            "evidence":{"start":1,"end":2,"text":"本"},
            "condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    service.begin_turn(&context, USER_MESSAGE).await;
    assert_eq!(harness.count("tool_selection_rules"), 0);
}
#[tokio::test]
pub(super) async fn g17_storage_failure_rolls_back_and_keeps_the_epoch() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let before = harness.epochs().rule;
    let parsed = ParsedExtraction {
        scenario: Scenario {
            intent: "x".into(),
            operation: Operation::Search,
            object_type: ObjectType::DecisionRecord,
            phase: Phase::Discover,
            input_kind: InputKind::Text,
        },
        accepted: vec![ExtractedFeedback {
            kind: FeedbackKind::ToolChoice,
            decision_id: Some("missing-decision".into()),
            rejected_tool_id: Some("web".into()),
            preferred_tool_id: Some("minutes".into()),
            scope: ScopeKind::Project,
            duration: Duration::Persistent,
            evidence: Evidence {
                start: 0,
                end: 6,
                text: "この案件".into(),
            },
            condition: FeedbackCondition {
                operation: Some(Operation::Search),
                object_type: Some(ObjectType::DecisionRecord),
                phase: None,
                input_kind: None,
            },
        }],
        rejected: Vec::new(),
    };
    let service = harness.service(NO_FEEDBACK);
    let result = service.apply_parsed(&context, &parsed);
    assert!(result.is_err());
    assert_eq!(harness.epochs().rule, before);
    assert_eq!(harness.count("tool_selection_rules"), 0);
}
#[tokio::test]
pub(super) async fn g18_deleting_the_source_message_removes_derived_rules() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message.clone()), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    assert!(harness.count("tool_selection_rules") > 0);
    write_tx(&harness.writer, {
        let message = message.clone();
        move |connection| {
            connection
                .execute(
                    "DELETE FROM conversation_messages WHERE id = ?1",
                    rusqlite::params![message],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        }
    })
    .expect("delete message");
    assert_eq!(harness.count("tool_selection_rules"), 0);
    assert_eq!(harness.count("tool_selection_feedback"), 0);
}
