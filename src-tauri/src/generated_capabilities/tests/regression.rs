use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn i07_import_shares_the_single_execution_slot() {
    let env = TestEnv::start(false);
    let permit = env
        .service
        .occupy_process_slot()
        .expect("the process slot is free");

    let error = env
        .import(CANDIDATE_A, ACCEPTANCE_A)
        .await
        .expect_err("import must not start a second host process");
    assert_eq!(error.code, CapabilityErrorCode::Busy);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_imports"),
        0,
        "a refused import records nothing"
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_revisions"),
        0
    );

    drop(permit);
    assert!(env.import(CANDIDATE_A, ACCEPTANCE_A).await.is_ok());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn l05_promotion_requires_the_current_runtime_and_an_intact_payload() {
    // (a) A revision verified under one runtime cannot be promoted by a service trusting a
    // different runtime, and cannot be invoked there either.
    let env = TestEnv::start(true);
    let revision = env.import_a().await;
    assert!(env.verify(&revision, ACCEPTANCE_A).await.unwrap().passed);

    let temporary = tempfile::tempdir().unwrap();
    let changed = temporary.path().join("runtime");
    let changed_digest = super::runtime_change::modified_runtime(&changed);
    assert_ne!(changed_digest, env.runtime_digest);
    let other = super::runtime_change::rebuild_service(&env, &changed);

    let epoch = other.catalog_epoch(&revision.capability_id).unwrap();
    let error = other
        .activate_revision(&revision.revision_id, epoch)
        .expect_err("a check from another runtime cannot promote the revision");
    assert_eq!(error.code, CapabilityErrorCode::NotValidated);
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capabilities WHERE current_revision_id IS NOT NULL"
        ),
        0
    );

    // An already-active revision must be re-verified before it runs on the new runtime.
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let temporary = tempfile::tempdir().unwrap();
    let changed = temporary.path().join("runtime");
    super::runtime_change::modified_runtime(&changed);
    let other = super::runtime_change::rebuild_service(&env, &changed);
    let resolved = other
        .resolve_active(&revision.capability_id)
        .expect("the active revision is visible");
    let error = other
        .invoke(
            super::super::service::InvokeRequest::new(
                resolved,
                "l05-runtime-change".into(),
                object(json!({ "enabled": true, "suspended": false })),
            ),
            &Cancellation::default(),
        )
        .await
        .expect_err("an active revision is not executed on an unverified runtime");
    assert_eq!(error.code, CapabilityErrorCode::NotValidated);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'succeeded'"),
        0
    );

    // (b) A payload modified after verification cannot be promoted.
    let env = TestEnv::start(true);
    let revision = env.import_a().await;
    assert!(env.verify(&revision, ACCEPTANCE_A).await.unwrap().passed);
    let target = env
        .service
        .store()
        .package_dir(&revision.package_hash)
        .join("request.json");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.push(b'\n');
    std::fs::write(&target, bytes).unwrap();
    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    let error = env
        .service
        .activate_revision(&revision.revision_id, epoch)
        .expect_err("a changed payload cannot be promoted");
    assert_eq!(error.code, CapabilityErrorCode::IntegrityError);
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Validated
    );
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capabilities WHERE current_revision_id IS NOT NULL"
        ),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn v05_a_revision_stopped_during_verification_leaves_no_running_check() {
    let env = TestEnv::start(false);
    let revision = env.import_a().await;
    assert!(env.verify(&revision, ACCEPTANCE_A).await.unwrap().passed);
    let runtime_digest = revision_row(&env, &revision.revision_id).runtime_digest;
    let inventory_hash = revision_row(&env, &revision.revision_id).inventory_hash;
    let acceptance_hash = revision_row(&env, &revision.revision_id).required_acceptance_hash;

    // A re-verification of a validated revision starts, then the revision is stopped while it runs.
    crate::generated_capabilities::lifecycle::transaction(&env.writer, |transaction| {
        crate::generated_capabilities::repository::insert_check(
            transaction,
            "mid-verification-check",
            &revision.revision_id,
            &runtime_digest,
            &inventory_hash,
            &acceptance_hash,
            "0",
        )
    })
    .expect("check starts");
    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    env.service
        .suspend_revision(&revision.revision_id, epoch)
        .expect("revision is stopped mid-verification");

    let validated = super::super::guards::finalize_check(
        &env.writer,
        &revision.revision_id,
        &super::super::guards::CheckOutcome {
            check_id: "mid-verification-check".into(),
            passed: true,
            error_code: None,
            runtime_digest,
            hashes: None,
            report_ref: "0".repeat(64),
        },
    )
    .expect("finalization runs");

    assert!(!validated, "a stopped revision is never promoted");
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Suspended
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_checks WHERE status = 'running'"),
        0,
        "the check reaches a terminal status"
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_checks WHERE status = 'failed'"),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn s04_an_abandoned_caller_does_not_leak_an_execution_registration() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");
    // Park the caller after it takes the process slot but before its call is accepted, then
    // abandon it. No registry entry or process slot may survive.
    let admission = env.service.acquire_admission().await;
    let service = env.service.clone();
    let call = tokio::spawn(async move {
        service
            .invoke(
                super::super::service::InvokeRequest::new(
                    resolved,
                    "abandoned-before-admission".into(),
                    object(json!({ "enabled": true, "suspended": false })),
                ),
                &Cancellation::default(),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    call.abort();
    let _ = call.await;
    drop(admission);
    assert_eq!(env.service.active_executions(), 0);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls"),
        0
    );
    assert!(env.service.occupy_process_slot().is_ok());
}
