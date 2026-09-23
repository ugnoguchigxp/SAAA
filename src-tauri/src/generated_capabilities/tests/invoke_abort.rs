use super::*;
use crate::generated_capabilities::service::InvokeRequest;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn s02_abort_cancels_started_process_and_releases_capacity() {
    let mut env = TestEnv::start(true);
    let temporary = tempfile::tempdir().unwrap();
    let (root, gate, marker) = super::hanging::install(temporary.path());
    env.service = super::runtime_change::rebuild_service(&env, &root);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env.service.resolve_active(&revision.capability_id).unwrap();
    fs::write(&gate, b"hang").unwrap();

    let service = env.service.clone();
    let call = tokio::spawn(async move {
        service
            .invoke(
                InvokeRequest::new(
                    resolved,
                    "aborted".into(),
                    object(json!({"enabled": true, "suspended": false})),
                ),
                &Cancellation::default(),
            )
            .await
    });
    let pid = super::hanging::await_child(&marker).await;
    assert_eq!(env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE id='aborted' AND status='running'"), 1);
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());

    super::hanging::await_settled(&env).await;
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE id='aborted' AND status='cancelled' AND error_code='cancelled'"),
        1
    );
    super::hanging::assert_reaped(pid);
    fs::remove_file(gate).unwrap();
    let result = env
        .service
        .invoke(
            InvokeRequest::new(
                env.service.resolve_active(&revision.capability_id).unwrap(),
                "after-abort".into(),
                object(json!({"enabled": true, "suspended": false})),
            ),
            &Cancellation::default(),
        )
        .await
        .unwrap();
    assert!(
        result.value,
        "the next real invocation can use the released slot"
    );
    assert!(env.service.shutdown());
}
