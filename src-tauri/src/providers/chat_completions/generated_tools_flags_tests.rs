//! C03: `tools=false` and `JsonProbe` must never offer or run a generated capability.

use super::generated_tools_tests::{
    history, ready_state, seeded_state, tool_call_stream, turn_input,
};
use super::*;
use crate::generated_capabilities::tests::TestEnv;

fn call_count(env: &TestEnv, revision_id: &str) -> i64 {
    env.writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM generated_capability_calls WHERE revision_id = ?1",
                    rusqlite::params![revision_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c03_tools_false_never_offers_or_runs_a_generated_tool() {
    let (env, state, revision_id, name) = ready_state().await;
    let run_id = "m2a-tools-off";
    let session = seeded_state(&env, &state, run_id);
    let arguments = json!({ "enabled": true, "suspended": false }).to_string();
    let (endpoint, server) = fixture(vec![(200, tool_call_stream(&name, &arguments), 0)]).await;
    let input = turn_input(run_id);
    let options = saaa_larm_session::http_api::LlmOptions {
        tools: false,
        ..Default::default()
    };
    let error = run_with_options(
        &endpoint,
        None,
        "fixture",
        &history(),
        10_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 1000,
            input: &input,
            on_event: &Sink::default(),
            cancellation: Arc::new(crate::RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
            }),
        },
        RequestMode::Stream,
        &options,
    )
    .await
    .expect_err("an unoffered tool call is a protocol error");
    assert_eq!(error, Error::failed(Failure::Protocol, false));
    assert!(server.await.unwrap()[0].get("tools").is_none());
    assert_eq!(
        call_count(&env, &revision_id),
        0,
        "tools=false must not run a generated call"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c03_json_probe_never_offers_generated_tools() {
    let (env, state, revision_id, _name) = ready_state().await;
    let run_id = "m2a-probe";
    let session = seeded_state(&env, &state, run_id);
    let body = json!({"model":"fixture","choices":[{"index":0,
        "message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]})
    .to_string();
    let (endpoint, server) = fixture(vec![(200, body, 0)]).await;
    let input = turn_input(run_id);
    let result = run_with_options(
        &endpoint,
        None,
        "fixture",
        &history(),
        5_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 64,
            input: &input,
            on_event: &Sink::default(),
            cancellation: Arc::new(crate::RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
            }),
        },
        RequestMode::JsonProbe,
        &saaa_larm_session::http_api::LlmOptions::standard(),
    )
    .await;
    assert_eq!(result, Ok("ok".into()));
    let requests = server.await.unwrap();
    assert_eq!(requests[0]["stream"], false);
    assert!(requests[0].get("tools").is_none());
    assert_eq!(
        call_count(&env, &revision_id),
        0,
        "JsonProbe must not run a generated call"
    );
}
