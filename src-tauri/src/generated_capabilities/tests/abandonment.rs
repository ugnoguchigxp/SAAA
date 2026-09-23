use super::*;
use crate::generated_capabilities::service::InvokeRequest;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn s01_shutdown_rejects_new_work_and_cancels_owned_executions() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");

    // An in-flight execution is signalled by shutdown; it releases the slot and its registry
    // entry only after observing the cancellation, exactly like a host process that is killed
    // and reaped.
    let owned = Cancellation::default();
    env.service.register_owned_execution("in-flight", &owned);
    let permit = env
        .service
        .occupy_process_slot()
        .expect("the process slot is free");
    let service = env.service.clone();
    let watcher = owned.clone();
    tokio::spawn(async move {
        while !watcher.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        drop(permit);
        service.unregister_owned_execution("in-flight");
    });

    assert!(
        env.service.shutdown(),
        "shutdown waits for the owned process slot to be released"
    );
    assert!(env.service.is_shutting_down());
    assert!(owned.is_cancelled(), "owned executions are cancelled");

    let error = env
        .service
        .invoke(
            InvokeRequest::new(
                resolved,
                "after-shutdown".into(),
                object(json!({ "enabled": true, "suspended": false })),
            ),
            &Cancellation::default(),
        )
        .await
        .expect_err("new calls are refused after shutdown");
    assert_eq!(error.code, CapabilityErrorCode::Unavailable);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE id = 'after-shutdown'"),
        0
    );

    let error = env
        .import(CANDIDATE_A, ACCEPTANCE_A)
        .await
        .expect_err("imports are refused after shutdown");
    assert_eq!(error.code, CapabilityErrorCode::Unavailable);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn s03_shutdown_cancels_a_running_import() {
    let env = TestEnv::start(false);
    let temporary = tempfile::tempdir().unwrap();
    let command = script_command(
        temporary.path(),
        "hang-import",
        "await Bun.stdin.text(); setInterval(() => {}, 1000);",
    );
    let candidate = env.candidate(CANDIDATE_A, ACCEPTANCE_A);
    let service = env.service.clone();
    let task = tokio::spawn(async move {
        service
            .import_candidate_with_command(&candidate, command)
            .await
    });

    // Wait until the import has actually reached the host inspect.
    let mut staging = 0;
    for _ in 0..300 {
        staging = env
            .scalar("SELECT COUNT(*) FROM generated_capability_imports WHERE status = 'staging'");
        if staging == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(staging, 1, "the import reached staging");

    assert!(
        env.service.shutdown(),
        "shutdown cancels the running inspect and drains the import"
    );
    let error = task
        .await
        .expect("import task joins")
        .expect_err("a cancelled import does not complete");
    assert_eq!(error.code, CapabilityErrorCode::Cancelled);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_imports WHERE status = 'staging'"),
        0,
        "the import record is no longer staging"
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_revisions"),
        0,
        "a cancelled import publishes nothing"
    );
}
