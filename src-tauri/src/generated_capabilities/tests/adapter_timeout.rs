//! X04 at the M2A adapter boundary: the adapter's own deadline must report `timeout`, cancel the
//! real host, write the terminal record and release the single process slot.

use super::*;
use crate::generated_capabilities::{publication::GeneratedToolSnapshot, tools};
use crate::runtime::agent_tools::AgentToolCall;
use std::time::Duration;

pub(super) async fn ready(env: &TestEnv) -> (RevisionRef, GeneratedToolSnapshot) {
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_publication(std::slice::from_ref(&revision.capability_id))
        .expect("publication resolves");
    let snapshot = GeneratedToolSnapshot::build(resolved).expect("snapshot builds");
    assert_eq!(snapshot.descriptors().len(), 1);
    (revision, snapshot)
}

pub(super) fn call(snapshot: &GeneratedToolSnapshot) -> AgentToolCall {
    AgentToolCall {
        id: "provider-call".into(),
        name: snapshot.descriptors()[0].tool_name.clone(),
        arguments: json!({ "enabled": true, "suspended": false }).to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn x04_the_adapter_deadline_times_out_cancels_and_settles() {
    let mut env = TestEnv::start(true);
    let temporary = tempfile::tempdir().unwrap();
    let (root, gate, marker) = super::hanging::install(temporary.path());
    env.service = super::runtime_change::rebuild_service(&env, &root);
    let (_revision, snapshot) = ready(&env).await;
    let call = call(&snapshot);
    fs::write(&gate, b"hang").unwrap();

    let service = env.service.clone();
    let run = crate::RunCancellation::default();
    let task = tokio::spawn(async move {
        tools::execute_with_actor(
            Some(service.as_ref()),
            &snapshot,
            &call,
            "conversation",
            None,
            Duration::from_secs(5),
            &run,
        )
        .await
    });
    let pid = super::hanging::await_child(&marker).await;
    let content = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("adapter deadline must fire")
        .expect("adapter task joins");
    let value: Value = serde_json::from_str(&content).expect("tool content is JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "timeout", "{content}");
    super::hanging::await_settled(&env).await;
    super::hanging::assert_reaped(pid);
    assert!(env.service.shutdown());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn x04b_expired_budget_is_rejected_before_any_call_row() {
    let env = TestEnv::start(true);
    let (_revision, snapshot) = ready(&env).await;
    let call = call(&snapshot);
    let before = env.scalar("SELECT COUNT(*) FROM generated_capability_calls");
    let run = crate::RunCancellation::default();
    let content = tools::execute_with_actor(
        Some(env.service.as_ref()),
        &snapshot,
        &call,
        "conversation",
        None,
        Duration::ZERO,
        &run,
    )
    .await;
    let value: Value = serde_json::from_str(&content).expect("tool content is JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "timeout", "{content}");
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls"),
        before,
        "an expired budget must not create a durable call row"
    );
}
