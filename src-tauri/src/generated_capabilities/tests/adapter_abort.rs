//! X01/X02 at the M2A adapter boundary: in-flight run cancellation and caller-future abort must
//! both settle the durable record and free the single process slot.

use super::*;
use crate::generated_capabilities::{publication::GeneratedToolSnapshot, tools};
use crate::runtime::agent_tools::AgentToolCall;
use std::sync::Arc;
use std::time::Duration;

async fn ready(env: &TestEnv) -> (RevisionRef, GeneratedToolSnapshot) {
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_publication(std::slice::from_ref(&revision.capability_id))
        .expect("publication resolves");
    let snapshot = GeneratedToolSnapshot::build(resolved).expect("snapshot builds");
    assert_eq!(snapshot.descriptors().len(), 1);
    (revision, snapshot)
}

fn call(snapshot: &GeneratedToolSnapshot) -> AgentToolCall {
    AgentToolCall {
        id: "provider-call".into(),
        name: snapshot.descriptors()[0].tool_name.clone(),
        arguments: json!({ "enabled": true, "suspended": false }).to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x02_aborting_the_adapter_future_cancels_and_settles() {
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
            Duration::from_secs(30),
            &run,
        )
        .await
    });
    let pid = super::hanging::await_child(&marker).await;
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'running'"),
        1
    );
    task.abort();
    let _ = task.await;
    super::hanging::await_settled(&env).await;
    super::hanging::assert_reaped(pid);
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capability_calls \
             WHERE status = 'cancelled' AND origin = 'conversation'"
        ),
        1
    );
    assert!(env.service.shutdown());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x01b_in_flight_run_cancellation_reports_cancelled_and_settles() {
    let mut env = TestEnv::start(true);
    let temporary = tempfile::tempdir().unwrap();
    let (root, gate, marker) = super::hanging::install(temporary.path());
    env.service = super::runtime_change::rebuild_service(&env, &root);
    let (_revision, snapshot) = ready(&env).await;
    let call = call(&snapshot);
    fs::write(&gate, b"hang").unwrap();

    let run = Arc::new(crate::RunCancellation::default());
    let signal = run.clone();
    let service = env.service.clone();
    let task = tokio::spawn(async move {
        tools::execute_with_actor(
            Some(service.as_ref()),
            &snapshot,
            &call,
            "conversation",
            None,
            Duration::from_secs(30),
            &run,
        )
        .await
    });
    let pid = super::hanging::await_child(&marker).await;
    signal.cancel();
    let content = task.await.expect("adapter task joins");
    let value: Value = serde_json::from_str(&content).expect("tool content is JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "cancelled", "{content}");
    super::hanging::await_settled(&env).await;
    super::hanging::assert_reaped(pid);
    assert!(env.service.shutdown());
}
