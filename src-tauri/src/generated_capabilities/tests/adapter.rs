//! E01-E06 and X01: the common generated-tool execution adapter.

use super::*;
use crate::generated_capabilities::publication::GeneratedToolSnapshot;
use crate::generated_capabilities::tools;
use crate::runtime::agent_tools::AgentToolCall;

async fn active_snapshot(env: &TestEnv) -> (RevisionRef, GeneratedToolSnapshot) {
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_publication(std::slice::from_ref(&revision.capability_id))
        .expect("publication resolves");
    let snapshot = GeneratedToolSnapshot::build(resolved).expect("snapshot builds");
    (revision, snapshot)
}

fn call(snapshot: &GeneratedToolSnapshot, id: &str, arguments: Value) -> AgentToolCall {
    AgentToolCall {
        id: id.to_string(),
        name: snapshot.descriptors()[0].tool_name.clone(),
        arguments: arguments.to_string(),
    }
}

async fn run(env: &TestEnv, snapshot: &GeneratedToolSnapshot, call: &AgentToolCall) -> String {
    tools::execute(
        Some(env.service.as_ref()),
        snapshot,
        call,
        "conversation",
        std::time::Duration::from_secs(5),
        &crate::RunCancellation::default(),
    )
    .await
}

fn content(result: &str) -> Value {
    serde_json::from_str(result).expect("tool content is JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e01_true_and_false_are_both_successful() {
    let env = TestEnv::start(true);
    let (_revision, snapshot) = active_snapshot(&env).await;
    for (enabled, suspended, expected) in [
        (true, false, true),
        (false, false, false),
        (true, true, false),
    ] {
        let result = run(
            &env,
            &snapshot,
            &call(
                &snapshot,
                "provider-call",
                json!({ "enabled": enabled, "suspended": suspended }),
            ),
        )
        .await;
        let value = content(&result);
        assert_eq!(value["ok"], true, "{result}");
        assert_eq!(value["value"], expected, "{result}");
        assert!(value["revisionId"].is_string());
        assert_ne!(value["callId"], "provider-call");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e02_invalid_inputs_never_start_a_host_process() {
    let env = TestEnv::start(true);
    let (revision, snapshot) = active_snapshot(&env).await;
    let before = env.call_count(&revision.revision_id);
    let invalid = [
        "not json".to_string(),
        "[]".to_string(),
        json!({ "enabled": true }).to_string(),
        json!({ "enabled": true, "suspended": false, "extra": true }).to_string(),
        json!({ "enabled": "true", "suspended": false }).to_string(),
        json!({ "enabled": null, "suspended": false }).to_string(),
        format!(
            r#"{{"enabled":true,"suspended":false,"pad":"{}"}}"#,
            "x".repeat(crate::generated_capabilities::publication::MAX_INPUT_BYTES)
        ),
    ];
    for arguments in invalid {
        let result = tools::execute(
            Some(env.service.as_ref()),
            &snapshot,
            &AgentToolCall {
                id: "provider-call".into(),
                name: snapshot.descriptors()[0].tool_name.clone(),
                arguments,
            },
            "conversation",
            std::time::Duration::from_secs(5),
            &crate::RunCancellation::default(),
        )
        .await;
        let value = content(&result);
        assert_eq!(value["ok"], false, "{result}");
        assert_eq!(value["error"]["code"], "invalid-input", "{result}");
    }
    assert_eq!(
        env.call_count(&revision.revision_id),
        before,
        "invalid input must not reach the host"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e03_an_unoffered_gc_name_is_refused() {
    let env = TestEnv::start(true);
    let (_revision, snapshot) = active_snapshot(&env).await;
    let forged = AgentToolCall {
        id: "provider-call".into(),
        name: "gc_00000000000000000000000000000000".into(),
        arguments: json!({ "enabled": true, "suspended": false }).to_string(),
    };
    let result = run(&env, &snapshot, &forged).await;
    let value = content(&result);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "invalid-input");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e04_provider_call_ids_never_become_the_durable_call_id() {
    let env = TestEnv::start(true);
    let (revision, snapshot) = active_snapshot(&env).await;
    let first = content(
        &run(
            &env,
            &snapshot,
            &call(
                &snapshot,
                "provider-call",
                json!({ "enabled": true, "suspended": false }),
            ),
        )
        .await,
    );
    let second = content(
        &run(
            &env,
            &snapshot,
            &call(
                &snapshot,
                "provider-call",
                json!({ "enabled": true, "suspended": false }),
            ),
        )
        .await,
    );
    assert_ne!(first["callId"], "provider-call");
    assert_ne!(second["callId"], "provider-call");
    assert_ne!(first["callId"], second["callId"]);
    assert_eq!(env.call_count(&revision.revision_id), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e05_an_offer_after_activation_is_rejected_not_forwarded() {
    let env = TestEnv::start(true);
    let (revision, snapshot) = active_snapshot(&env).await;
    let b = env.import_b().await;
    assert!(env.verify(&b, ACCEPTANCE_B).await.unwrap().passed);
    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    env.service
        .activate_revision(&b.revision_id, epoch)
        .expect("B activates");

    let result = run(
        &env,
        &snapshot,
        &call(
            &snapshot,
            "provider-call",
            json!({ "enabled": true, "suspended": false }),
        ),
    )
    .await;
    let value = content(&result);
    assert_eq!(value["error"]["code"], "stale-revision", "{result}");

    // The next offer resolves the new active revision.
    let next = env
        .service
        .resolve_publication(std::slice::from_ref(&revision.capability_id))
        .unwrap();
    assert_eq!(next[0].revision_id, b.revision_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e06_suspension_and_tampering_are_refused_without_leaking_internals() {
    let env = TestEnv::start(true);
    let (revision, snapshot) = active_snapshot(&env).await;
    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    env.service
        .suspend_revision(&revision.revision_id, epoch)
        .expect("suspension succeeds");
    let stopped = run(
        &env,
        &snapshot,
        &call(
            &snapshot,
            "provider-call",
            json!({ "enabled": true, "suspended": false }),
        ),
    )
    .await;
    assert_eq!(content(&stopped)["error"]["code"], "stale-revision");

    let env = TestEnv::start(true);
    let (revision, snapshot) = active_snapshot(&env).await;
    let target = env
        .service
        .store()
        .package_dir(&revision.package_hash)
        .join("request.json");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.push(b'\n');
    std::fs::write(&target, bytes).unwrap();
    let tampered = run(
        &env,
        &snapshot,
        &call(
            &snapshot,
            "provider-call",
            json!({ "enabled": true, "suspended": false }),
        ),
    )
    .await;
    let value = content(&tampered);
    assert_eq!(value["error"]["code"], "integrity-error", "{tampered}");
    for secret in ["request.json", "/packages", "SQL", "stderr"] {
        assert!(!tampered.contains(secret), "leaked {secret}: {tampered}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x01_a_pre_cancelled_run_never_starts_a_host_process() {
    let env = TestEnv::start(true);
    let (revision, snapshot) = active_snapshot(&env).await;
    let cancellation = crate::RunCancellation::default();
    cancellation.cancel();
    let result = tools::execute(
        Some(env.service.as_ref()),
        &snapshot,
        &call(
            &snapshot,
            "provider-call",
            json!({ "enabled": true, "suspended": false }),
        ),
        "conversation",
        std::time::Duration::from_secs(5),
        &cancellation,
    )
    .await;
    let value = content(&result);
    assert_eq!(value["error"]["code"], "cancelled", "{result}");
    assert_eq!(env.call_count(&revision.revision_id), 0);
}
