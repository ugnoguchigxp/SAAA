use super::*;
use crate::generated_capabilities::service::InvokeRequest;

fn truth_table() -> [(bool, bool, bool); 4] {
    [
        (false, false, false),
        (false, true, false),
        (true, false, true),
        (true, true, false),
    ]
}

async fn invoke(
    env: &TestEnv,
    resolved: &super::super::service::ResolvedCapability,
    enabled: bool,
    suspended: bool,
) -> CapabilityResult<super::super::service::InvocationResult> {
    env.service
        .invoke(
            InvokeRequest::new(
                resolved.clone(),
                format!("call-{}-{}", enabled as u8, suspended as u8),
                object(json!({ "enabled": enabled, "suspended": suspended })),
            ),
            &Cancellation::default(),
        )
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn i01_truth_table_matches_and_invalid_inputs_never_spawn() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");
    for (enabled, suspended, expected) in truth_table() {
        let result = invoke(&env, &resolved, enabled, suspended)
            .await
            .expect("invocation succeeds");
        assert_eq!(result.value, expected, "{enabled}/{suspended}");
        assert_eq!(result.revision_id, revision.revision_id);
        assert_eq!(result.package_hash, revision.package_hash);
    }
    let calls_after_valid: i64 = env.call_count(&revision.revision_id);

    let invalid = [
        object(json!({ "enabled": "true", "suspended": false })),
        object(json!({ "enabled": true })),
        object(json!({ "enabled": true, "suspended": false, "extra": true })),
        object(json!({ "enabled": null, "suspended": false })),
    ];
    for (index, input) in invalid.into_iter().enumerate() {
        let error = env
            .service
            .invoke(
                InvokeRequest::new(resolved.clone(), format!("invalid-{index}"), input),
                &Cancellation::default(),
            )
            .await
            .expect_err("invalid input is refused");
        assert_eq!(error.code, CapabilityErrorCode::InvalidInput);
    }
    assert_eq!(
        env.call_count(&revision.revision_id),
        calls_after_valid,
        "invalid input produced no host calls"
    );
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'succeeded' AND result_bool = 0"
        ),
        3,
        "each normal false is stored as a successful call with result_bool = 0"
    );
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'succeeded' AND result_bool = 1"
        ),
        1,
        "each true is stored as a successful call with result_bool = 1"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn i02_stale_resolved_capability_is_not_forwarded() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let stale = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");

    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    env.service
        .suspend_revision(&revision.revision_id, epoch)
        .expect("suspend succeeds");

    let error = invoke(&env, &stale, true, false)
        .await
        .expect_err("stale resolution is refused");
    assert_eq!(error.code, CapabilityErrorCode::StaleRevision);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'succeeded'"),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn i03_suspension_does_not_cancel_accepted_work() {
    let env = TestEnv::start(true);
    let a = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&a.capability_id)
        .expect("capability is active");
    let epoch = env.service.catalog_epoch(&a.capability_id).unwrap();

    // Suspension before acceptance refuses the call.
    env.service
        .suspend_revision(&a.revision_id, epoch)
        .expect("suspend succeeds");
    let error = invoke(&env, &resolved, true, false)
        .await
        .expect_err("suspended capability is refused");
    assert_eq!(error.code, CapabilityErrorCode::StaleRevision);
    assert_eq!(env.revision_state(&a.revision_id), RevisionState::Suspended);
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capabilities WHERE current_revision_id IS NOT NULL"
        ),
        0
    );

    // A capability that is still active keeps executing after another revision is suspended.
    let env = TestEnv::start(true);
    let a = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let b = env
        .import(CANDIDATE_B, ACCEPTANCE_B)
        .await
        .expect("B imports");
    assert!(env.verify(&b, ACCEPTANCE_B).await.unwrap().passed);
    let epoch = env.service.catalog_epoch(&a.capability_id).unwrap();
    env.service
        .suspend_revision(&b.revision_id, epoch)
        .expect("suspending a non-current validated revision succeeds");
    let resolved = env
        .service
        .resolve_active(&a.capability_id)
        .expect("A remains active");
    assert!(invoke(&env, &resolved, true, false).await.unwrap().value);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn i04_cancelled_calls_are_recorded_and_release_capacity() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");

    let cancellation = Cancellation::default();
    cancellation.cancel();
    let error = env
        .service
        .invoke(
            InvokeRequest::new(
                resolved.clone(),
                "cancelled".into(),
                object(json!({ "enabled": true, "suspended": false })),
            ),
            &cancellation,
        )
        .await
        .expect_err("cancelled call fails");
    assert_eq!(error.code, CapabilityErrorCode::Cancelled);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'cancelled'"),
        1
    );

    // The process slot is released for the next call.
    let result = invoke(&env, &resolved, true, false)
        .await
        .expect("next call succeeds");
    assert!(result.value);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn i05_capacity_is_bounded_and_never_queues() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");
    let permit = env
        .service
        .occupy_process_slot()
        .expect("process slot is free");
    let error = invoke(&env, &resolved, true, false)
        .await
        .expect_err("capacity is exhausted");
    assert_eq!(error.code, CapabilityErrorCode::Busy);
    assert_eq!(
        env.call_count(&revision.revision_id),
        0,
        "no call was accepted"
    );
    drop(permit);
    assert!(invoke(&env, &resolved, true, false).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn i06_database_write_failures_never_report_success() {
    // Start-of-call failure: no host process may be spawned.
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");
    env.writer
        .write(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER no_call_insert BEFORE INSERT ON generated_capability_calls
                     BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let error = invoke(&env, &resolved, true, false)
        .await
        .expect_err("call acceptance fails");
    assert_eq!(error.code, CapabilityErrorCode::StorageError);
    assert_eq!(env.call_count(&revision.revision_id), 0);

    // End-of-call failure: the success value must not be returned.
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");
    env.writer
        .write(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER no_call_update BEFORE UPDATE ON generated_capability_calls
                     BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let error = invoke(&env, &resolved, true, false)
        .await
        .expect_err("call finalization fails");
    assert_eq!(error.code, CapabilityErrorCode::StorageError);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'running'"),
        1,
        "the call stays running for startup recovery"
    );
}
