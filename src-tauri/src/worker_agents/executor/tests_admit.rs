use super::test_fakes::*;
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::now_ms;
use serde_json::json;

fn failed_code(outcome: WorkerOutcome) -> FailureCode {
    match outcome {
        WorkerOutcome::Failed { failure, .. } => failure.code,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn double_admit_with_identical_input_returns_the_same_task() {
    let harness = Harness::build(Setup::new());
    let first = harness.admit_task_id();
    // Same input with reordered keys is the same canonical input.
    let second = harness
        .admit_with(AGENT, json!({"query": "桜"}), now_ms() + 120_000)
        .unwrap();
    assert_eq!(second, WorkerOutcome::Pending { task_id: first });
    assert_eq!(harness.count("SELECT count(*) FROM worker_tasks"), 1);
    assert_eq!(
        harness.count("SELECT count(*) FROM task_queue_jobs WHERE lane='worker'"),
        1
    );
}

#[test]
fn a_task_pins_the_offered_revision_and_the_limits() {
    let harness = Harness::build(Setup::new());
    let task = harness.task(&harness.admit_task_id());
    assert_eq!(task.profile_revision_id, harness.revision_id);
    assert_eq!(task.state, "accepted");
    assert_eq!(task.delivery, "sync_waiting");
    assert!(task.deadline_at_ms > now_ms() + 30_000);
}

#[test]
fn input_that_is_not_a_persisted_user_message_is_rejected() {
    let harness = Harness::build(Setup::new());
    harness.exec_sql(&format!(
        "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
         VALUES('msg_assistant', '{}', 'assistant', 'hi', '1')",
        crate::PRIMARY_CONVERSATION_ID
    ));
    // Rebind the decision-checked request to a message that is not a user message.
    let delegate = DelegateRequest {
        agent: AGENT.into(),
        input: json!({"query": "x"}),
    };
    for message in ["missing_message", "msg_assistant"] {
        let request = AdmitRequest {
            conversation_id: crate::PRIMARY_CONVERSATION_ID,
            input_message_id: message,
            origin_job_key: "job_1",
            decision_id: "dec_1",
            delegate: &delegate,
            conversation_deadline_ms: now_ms() + 60_000,
        };
        let result = harness
            .writer
            .transact(|c| super::admit(c, &request, now_ms()));
        assert!(result.is_err(), "{message} must be rejected");
    }
    assert_eq!(harness.count("SELECT count(*) FROM worker_tasks"), 0);
}

#[test]
fn an_agent_that_was_not_offered_is_refused() {
    let harness = Harness::build(Setup::new());
    let unknown = harness
        .admit_with("other_agent", json!({"query": "x"}), now_ms() + 60_000)
        .unwrap();
    assert_eq!(failed_code(unknown), FailureCode::NoMatchingAgent);
    harness.exec_sql("UPDATE worker_discovery_candidates SET offered = 0");
    let not_offered = harness
        .admit_with(AGENT, json!({"query": "x"}), now_ms() + 60_000)
        .unwrap();
    assert_eq!(failed_code(not_offered), FailureCode::NoMatchingAgent);
    assert_eq!(harness.count("SELECT count(*) FROM worker_tasks"), 0);
}

#[test]
fn a_changed_epoch_makes_the_offer_stale() {
    let harness = Harness::build(Setup::new());
    harness.exec_sql("UPDATE worker_meta SET registry_epoch = registry_epoch + 1");
    assert_eq!(failed_code(harness.admit()), FailureCode::StaleOffer);
    harness.exec_sql("UPDATE worker_meta SET registry_epoch = 0, acl_epoch = acl_epoch + 1");
    assert_eq!(failed_code(harness.admit()), FailureCode::StaleOffer);
    assert_eq!(harness.count("SELECT count(*) FROM worker_tasks"), 0);
}

#[test]
fn a_disabled_profile_or_replaced_revision_is_stale() {
    let harness = Harness::build(Setup::new());
    harness.exec_sql("UPDATE worker_profiles SET enabled = 0");
    assert_eq!(failed_code(harness.admit()), FailureCode::StaleOffer);
    harness.exec_sql("UPDATE worker_profiles SET enabled = 1, current_revision_id = NULL");
    assert_eq!(failed_code(harness.admit()), FailureCode::StaleOffer);
}

#[test]
fn input_that_violates_the_schema_is_invalid() {
    let harness = Harness::build(Setup::new());
    for input in [
        json!({}),
        json!({"query": 3}),
        json!({"query": "x", "extra": 1}),
    ] {
        let outcome = harness.admit_with(AGENT, input, now_ms() + 60_000).unwrap();
        assert_eq!(failed_code(outcome), FailureCode::InputInvalid);
    }
    assert_eq!(harness.count("SELECT count(*) FROM worker_tasks"), 0);
}

#[test]
fn the_sync_wait_is_capped_by_the_conversation_deadline() {
    let harness = Harness::build(Setup::new());
    let before = now_ms();
    let outcome = harness
        .admit_with(AGENT, json!({"query": "桜"}), before + 6_000)
        .unwrap();
    let WorkerOutcome::Pending { task_id } = outcome else {
        panic!("pending expected")
    };
    let wait: i64 = harness.count(&format!(
        "SELECT sync_wait_until_ms - created_at_ms FROM worker_tasks WHERE id='{task_id}'"
    ));
    assert!((0..=1_100).contains(&wait), "sync wait was {wait}ms");
}
