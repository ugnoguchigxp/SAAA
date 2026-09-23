use super::*;
use crate::generated_capabilities::recovery;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn r01_interrupted_rows_are_recovered_without_spawning() {
    let env = TestEnv::start(true);
    let revision = env.import_a().await;
    let revision_id = revision.revision_id.clone();
    env.writer
        .write(move |connection| {
            connection
                .execute(
                    "INSERT INTO generated_capability_checks(id, revision_id, runtime_digest, inventory_hash, acceptance_hash, status, started_at)
                     VALUES ('left-check', ?1, '0', '0', '0', 'running', '0')",
                    rusqlite::params![revision_id],
                )
                .map_err(crate::database_error)?;
            connection
                .execute(
                    "INSERT INTO generated_capability_calls(id, revision_id, package_hash, origin, status, started_at)
                     VALUES ('left-call', ?1, '0', 'internal', 'running', '0')",
                    rusqlite::params![revision_id],
                )
                .map_err(crate::database_error)?;
            connection
                .execute(
                    "INSERT INTO generated_capability_imports(id, status, created_at)
                     VALUES ('left-import', 'staging', '0')",
                    [],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();

    let summary = recovery::reconcile_startup(&env.service).expect("recovery runs");
    assert_eq!(summary.interrupted_checks, 1);
    assert_eq!(summary.interrupted_calls, 1);
    assert_eq!(summary.interrupted_imports, 1);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_checks WHERE status = 'interrupted'"),
        1
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'interrupted'"),
        1
    );
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capability_imports WHERE status = 'interrupted'"
        ),
        1
    );
    // Recovery performs no retry and no candidate execution.
    assert_eq!(env.call_count(&revision.revision_id), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn r02_missing_or_changed_payload_stops_the_capability() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let epoch_before = env.service.catalog_epoch(&revision.capability_id).unwrap();

    std::fs::remove_dir_all(env.service.store().package_dir(&revision.package_hash)).unwrap();
    let summary = recovery::reconcile_startup(&env.service).expect("recovery runs");
    assert_eq!(
        summary.missing_packages,
        vec![revision.package_hash.clone()]
    );
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Suspended
    );
    assert_eq!(
        env.service.catalog_epoch(&revision.capability_id).unwrap(),
        epoch_before + 1
    );
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM generated_capabilities WHERE current_revision_id IS NOT NULL"
        ),
        0
    );
    assert_eq!(
        env.service
            .resolve_active(&revision.capability_id)
            .expect_err("no active revision")
            .code,
        CapabilityErrorCode::NotActive
    );

    // A tampered payload is treated the same way.
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let target = env
        .service
        .store()
        .package_dir(&revision.package_hash)
        .join("request.json");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.push(b'\n');
    std::fs::write(&target, bytes).unwrap();
    let summary = recovery::reconcile_startup(&env.service).expect("recovery runs");
    assert_eq!(
        summary.missing_packages,
        vec![revision.package_hash.clone()]
    );
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Suspended
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn r03_packages_without_a_catalog_row_are_reported_not_published() {
    let env = TestEnv::start(true);
    let orphan = env.service.store().root().join("packages").join("deadbeef");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("capability.json"), b"{}").unwrap();
    let summary = recovery::reconcile_startup(&env.service).expect("recovery runs");
    assert_eq!(summary.orphan_packages, vec!["deadbeef".to_string()]);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_revisions"),
        0,
        "an orphan package is never registered automatically"
    );
    assert!(
        orphan.exists(),
        "an orphan package is never deleted automatically"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn r04_restart_keeps_the_catalog_without_running_candidates() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let epoch = env.service.catalog_epoch(&revision.capability_id).unwrap();
    let calls = env.call_count(&revision.revision_id);

    for _ in 0..2 {
        let summary = recovery::reconcile_startup(&env.service).expect("recovery runs");
        assert!(summary.missing_packages.is_empty());
        assert!(summary.orphan_packages.is_empty());
        assert_eq!(summary.interrupted_checks, 0);
        assert_eq!(summary.interrupted_calls, 0);
    }
    assert_eq!(
        env.revision_state(&revision.revision_id),
        RevisionState::Active
    );
    assert_eq!(
        env.service
            .resolve_active(&revision.capability_id)
            .unwrap()
            .revision_id,
        revision.revision_id
    );
    assert_eq!(
        env.service.catalog_epoch(&revision.capability_id).unwrap(),
        epoch
    );
    assert_eq!(env.call_count(&revision.revision_id), calls);
}
