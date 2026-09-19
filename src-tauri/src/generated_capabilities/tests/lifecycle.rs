use super::*;

fn hash_columns(
    env: &TestEnv,
    revision_id: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    env.writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT source_hash, program_hash, artifact_hash FROM generated_capability_revisions WHERE id = ?1",
                    rusqlite::params![revision_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(crate::database_error)
        })
        .expect("hash columns")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v01_a_valid_candidate_becomes_validated_with_recorded_hashes() {
    let env = TestEnv::start(false);
    let revision = env.import_a().await;
    let report = env
        .verify(&revision, ACCEPTANCE_A)
        .await
        .expect("verify runs");
    assert!(report.passed, "{}", report.detail);
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Validated
    );
    let (source, program, artifact) = hash_columns(&env, &revision.revision_id);
    assert!(source.is_some() && program.is_some() && artifact.is_some());
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_checks WHERE status = 'passed'"),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v02_mismatched_acceptance_never_validates() {
    // (a) A is imported with B's expectations: the acceptance cases disagree.
    let env = TestEnv::start(false);
    let revision = env
        .import(CANDIDATE_A, ACCEPTANCE_B)
        .await
        .expect("import succeeds");
    let report = env
        .verify(&revision, ACCEPTANCE_B)
        .await
        .expect("verify runs");
    assert!(!report.passed, "mismatched acceptance must fail");
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Candidate
    );

    // (b) A is imported with A's expectations; verifying with B's reference is refused.
    let env = TestEnv::start(false);
    let revision = env.import_a().await;
    let error = env
        .verify(&revision, ACCEPTANCE_B)
        .await
        .expect_err("acceptance hash mismatch is refused");
    assert_eq!(error.code, CapabilityErrorCode::IntegrityError);
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Candidate
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_checks"),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v03_double_verify_and_suspended_revisions_are_refused() {
    let env = TestEnv::start(false);
    let revision = env.import_a().await;

    // A running check for the same revision makes a second verification busy.
    let revision_id = revision.revision_id.clone();
    let inventory_hash = revision_row(&env, &revision_id).inventory_hash;
    let acceptance_hash = revision_row(&env, &revision_id).required_acceptance_hash;
    let runtime_digest = env.runtime_digest.clone();
    env.writer
        .write(move |connection| {
            connection
                .execute(
                    "INSERT INTO generated_capability_checks(id, revision_id, runtime_digest, inventory_hash, acceptance_hash, status, started_at)
                     VALUES ('running-check', ?1, ?2, ?3, ?4, 'running', '0')",
                    rusqlite::params![
                        revision_id,
                        runtime_digest,
                        inventory_hash,
                        acceptance_hash,
                    ],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let error = env
        .verify(&revision, ACCEPTANCE_A)
        .await
        .expect_err("second verification is busy");
    assert_eq!(error.code, CapabilityErrorCode::Busy);

    // A suspended revision is not verified, and its state is never lifted by a passing check.
    env.writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE generated_capability_checks SET status = 'interrupted' WHERE id = 'running-check'",
                    [],
                )
                .map_err(crate::database_error)?;
            connection
                .execute(
                    "UPDATE generated_capability_revisions SET state = 'suspended' WHERE id = ?1",
                    rusqlite::params![revision.revision_id],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let error = env
        .verify(&revision, ACCEPTANCE_A)
        .await
        .expect_err("suspended revisions are refused");
    assert_eq!(error.code, CapabilityErrorCode::NotValidated);
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Suspended
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v04_cancellation_aborts_verification_without_validating() {
    let env = TestEnv::start(false);
    let revision = env.import_a().await;
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let report = env
        .service
        .verify_candidate(&revision.revision_id, ACCEPTANCE_A, &cancellation)
        .await
        .expect("verification is reported, not thrown");
    assert!(!report.passed, "cancelled verification is never a pass");
    assert_eq!(report.error_code, Some(CapabilityErrorCode::Cancelled));
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Candidate
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_checks WHERE status = 'passed'"),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l01_activation_requires_a_passed_check_for_the_current_runtime() {
    let env = TestEnv::start(true);
    let revision = env.import_a().await;

    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    let error = env
        .service
        .activate_revision(&revision.revision_id, epoch)
        .expect_err("unverified revision is refused");
    assert_eq!(error.code, CapabilityErrorCode::NotValidated);

    let report = env.verify(&revision, ACCEPTANCE_A).await.unwrap();
    assert!(report.passed);

    // A runtime change invalidates the earlier passing check.
    env.writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE generated_capability_revisions SET runtime_digest = ?2 WHERE id = ?1",
                    rusqlite::params![revision.revision_id, "1".repeat(64)],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    let error = env
        .service
        .activate_revision(&revision.revision_id, epoch)
        .expect_err("stale check is refused");
    assert_eq!(error.code, CapabilityErrorCode::NotValidated);
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capabilities WHERE current_revision_id IS NOT NULL"
        ),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l02_switching_to_a_passing_candidate_keeps_epoch_and_states() {
    let env = TestEnv::start(true);
    let a = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let epoch_before = env.service.catalog_epoch(&a.capability_id).unwrap();
    let b = env
        .import(CANDIDATE_B, ACCEPTANCE_B)
        .await
        .expect("B imports");
    let report = env.verify(&b, ACCEPTANCE_B).await.unwrap();
    assert!(report.passed, "{}", report.detail);

    let activation = env
        .service
        .activate_revision(&b.revision_id, epoch_before)
        .expect("B activates");
    assert_eq!(
        activation.previous_revision_id.as_deref(),
        Some(a.revision_id.as_str())
    );
    assert_eq!(activation.catalog_epoch, epoch_before + 1);
    assert_eq!(env.revision_state(&b.revision_id), RevisionState::Active);
    assert_eq!(env.revision_state(&a.revision_id), RevisionState::Validated);
    assert_eq!(
        env.service
            .resolve_active(&a.capability_id)
            .unwrap()
            .revision_id,
        b.revision_id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l03_two_activations_at_the_same_epoch_conflict() {
    let env = TestEnv::start(true);
    let a = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let epoch = env.service.catalog_epoch(&a.capability_id).unwrap();
    let b = env
        .import(CANDIDATE_B, ACCEPTANCE_B)
        .await
        .expect("B imports");
    assert!(env.verify(&b, ACCEPTANCE_B).await.unwrap().passed);

    env.service
        .activate_revision(&b.revision_id, epoch)
        .expect("first activation succeeds");
    let error = env
        .service
        .activate_revision(&a.revision_id, epoch)
        .expect_err("second activation conflicts");
    assert_eq!(error.code, CapabilityErrorCode::Conflict);
    assert_eq!(env.revision_state(&b.revision_id), RevisionState::Active);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l04_transaction_failure_rolls_back_pointers_states_and_epoch() {
    let env = TestEnv::start(true);
    let a = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let epoch_before = env.service.catalog_epoch(&a.capability_id).unwrap();

    env.writer
        .write(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER injected_failure BEFORE UPDATE ON generated_capabilities
                     BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();

    let b = env
        .import(CANDIDATE_B, ACCEPTANCE_B)
        .await
        .expect("B imports");
    assert!(env.verify(&b, ACCEPTANCE_B).await.unwrap().passed);
    let error = env
        .service
        .activate_revision(&b.revision_id, epoch_before)
        .expect_err("injected failure aborts activation");
    assert_eq!(error.code, CapabilityErrorCode::StorageError);
    assert_eq!(
        env.revision_state(&b.revision_id),
        RevisionState::Validated,
        "the new revision was not activated"
    );
    assert_eq!(env.revision_state(&a.revision_id), RevisionState::Active);
    assert_eq!(
        env.service.catalog_epoch(&a.capability_id).unwrap(),
        epoch_before,
        "epoch rolled back"
    );
    assert_eq!(
        env.service
            .resolve_active(&a.capability_id)
            .unwrap()
            .revision_id,
        a.revision_id
    );
}
