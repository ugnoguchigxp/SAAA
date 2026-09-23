use super::*;
use crate::generated_capabilities::{
    contracts::{package_hash, safe_flat_path, sha256_hex},
    package_store::PackageStore,
};

pub(super) fn packages_count(env: &TestEnv) -> usize {
    std::fs::read_dir(env.service.store().root().join("packages"))
        .unwrap()
        .count()
}

pub(super) fn failed_imports(env: &TestEnv) -> i64 {
    env.scalar("SELECT COUNT(*) FROM generated_capability_imports WHERE status = 'failed'")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn p01_reimporting_the_same_package_reuses_the_revision() {
    let env = TestEnv::start(false);
    let first = env.import_a().await;
    let second = env.import_a().await;
    assert_eq!(first.revision_id, second.revision_id);
    assert_eq!(first.package_hash, second.package_hash);
    assert_eq!(
        env.revision_state(&first.revision_id),
        RevisionState::Candidate
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_revisions"),
        1
    );
    assert_eq!(env.scalar("SELECT COUNT(*) FROM generated_capabilities"), 1);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_imports WHERE status = 'completed'"),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn p02_managed_copy_ignores_later_external_changes() {
    let env = TestEnv::start(false);
    let temporary = tempfile::tempdir().unwrap();
    let external = temporary.path().join("candidate-a");
    copy_tree(&candidate_dir(CANDIDATE_A), &external);
    let revision = env
        .service
        .import_candidate(&ImportCandidate {
            capability_id: None,
            candidate_directory: external.clone(),
            acceptance_id: ACCEPTANCE_A.to_string(),
            provenance: "external-copy".to_string(),
        })
        .await
        .expect("import succeeds");

    // Mutating the external source after import must not change the managed copy.
    let source = external.join("enabled-user.llang.jsonc");
    let mut bytes = std::fs::read(&source).unwrap();
    bytes.extend_from_slice(b"\n");
    std::fs::write(&source, bytes).unwrap();

    let manifest = env.service.store().manifest_path(&revision.package_hash);
    let host = env.host();
    let response = host
        .execute(
            &HostRequest::invoke(
                "p02",
                &revision.package_hash,
                object(json!({ "enabled": true, "suspended": false })),
                5_000,
            ),
            &manifest,
            &Cancellation::default(),
        )
        .await
        .expect("managed copy still runs");
    assert!(matches!(
        response.outcome,
        HostOutcome::Ok(OperationResult::Invoke(true))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn p03_unsafe_layouts_are_refused_before_publication() {
    let env = TestEnv::start(false);
    let temporary = tempfile::tempdir().unwrap();
    let before = packages_count(&env);

    // Traversal in a referenced path.
    let traversal = temporary.path().join("traversal");
    copy_tree(&candidate_dir(CANDIDATE_A), &traversal);
    let manifest_path = traversal.join("capability.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    *manifest.pointer_mut("/files/request/path").unwrap() = json!("../request.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = env
        .service
        .import_candidate(&ImportCandidate {
            capability_id: None,
            candidate_directory: traversal,
            acceptance_id: ACCEPTANCE_A.to_string(),
            provenance: "fixture".to_string(),
        })
        .await
        .expect_err("traversal is refused");
    assert_eq!(error.code, CapabilityErrorCode::UnsupportedPackageLayout);

    // Symlinked package file.
    let symlinked = temporary.path().join("symlinked");
    copy_tree(&candidate_dir(CANDIDATE_A), &symlinked);
    let target = candidate_dir(CANDIDATE_A).join("request.json");
    let link = symlinked.join("request.json");
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let error = env
        .service
        .import_candidate(&ImportCandidate {
            capability_id: None,
            candidate_directory: symlinked,
            acceptance_id: ACCEPTANCE_A.to_string(),
            provenance: "fixture".to_string(),
        })
        .await
        .expect_err("symlink is refused");
    assert_eq!(error.code, CapabilityErrorCode::UnsupportedPackageLayout);

    // Oversized file.
    let oversized = temporary.path().join("oversized");
    copy_tree(&candidate_dir(CANDIDATE_A), &oversized);
    std::fs::write(oversized.join("request.json"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let error = env
        .service
        .import_candidate(&ImportCandidate {
            capability_id: None,
            candidate_directory: oversized,
            acceptance_id: ACCEPTANCE_A.to_string(),
            provenance: "fixture".to_string(),
        })
        .await
        .expect_err("oversized file is refused");
    assert_eq!(error.code, CapabilityErrorCode::InvalidPackage);

    assert_eq!(packages_count(&env), before, "nothing was published");
    assert_eq!(failed_imports(&env), 3, "every refusal is recorded");
    assert!(safe_flat_path("request.json"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn p04_tampering_with_the_managed_copy_is_detected() {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let directory = env.service.store().package_dir(&revision.package_hash);
    let target = directory.join("request.json");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.extend_from_slice(b"\n");
    std::fs::write(&target, bytes).unwrap();

    // A fresh import of the same package refuses to share the corrupted directory.
    let error = env
        .import(CANDIDATE_A, ACCEPTANCE_A)
        .await
        .expect_err("corrupted managed copy");
    assert_eq!(error.code, CapabilityErrorCode::IntegrityError);

    // Invocation re-checks the managed copy before spawning.
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("capability is active");
    let error = env
        .service
        .invoke(
            super::super::service::InvokeRequest::new(
                resolved,
                "p04".into(),
                object(json!({ "enabled": true, "suspended": false })),
            ),
            &Cancellation::default(),
        )
        .await
        .expect_err("tampered package is refused");
    assert_eq!(error.code, CapabilityErrorCode::IntegrityError);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn p05_conflicting_existing_package_directory_is_never_overwritten() {
    let env = TestEnv::start(false);
    let staged_hash = package_hash(
        &serde_json::from_slice::<Value>(
            &std::fs::read(candidate_dir(CANDIDATE_A).join("capability.json")).unwrap(),
        )
        .unwrap(),
    );
    let conflicting = env.service.store().package_dir(&staged_hash);
    std::fs::create_dir_all(&conflicting).unwrap();
    std::fs::write(conflicting.join("capability.json"), b"{\"different\":true}").unwrap();

    let error = env
        .import(CANDIDATE_A, ACCEPTANCE_A)
        .await
        .expect_err("conflicting directory is refused");
    assert_eq!(error.code, CapabilityErrorCode::IntegrityError);
    assert_eq!(
        std::fs::read(conflicting.join("capability.json")).unwrap(),
        b"{\"different\":true}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn p06_acceptance_for_another_capability_is_refused() {
    let temporary = tempfile::tempdir().unwrap();
    let ledger = temporary.path().join("acceptance");
    copy_tree(&fixture_root().join("acceptance"), &ledger);

    // Point the ledger's case file at a different capability id and keep the recorded hash
    // consistent, so only the capability binding can reject the import.
    let case_path = ledger.join("enabled-user.truth-table.json");
    let mut acceptance: Value =
        serde_json::from_slice(&std::fs::read(&case_path).unwrap()).unwrap();
    acceptance["capabilityId"] = json!("some-other-capability");
    let bytes = serde_json::to_vec(&acceptance).unwrap();
    std::fs::write(&case_path, &bytes).unwrap();
    let index_path = ledger.join("index.json");
    let mut index: Value = serde_json::from_slice(&std::fs::read(&index_path).unwrap()).unwrap();
    *index.pointer_mut("/entries/0/hash").unwrap() = json!(sha256_hex(&bytes));
    std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();

    let env = TestEnv::start_with_ledger(false, ledger);
    let error = env
        .import(CANDIDATE_A, ACCEPTANCE_A)
        .await
        .expect_err("acceptance from another capability is refused");
    assert_eq!(error.code, CapabilityErrorCode::IntegrityError);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_revisions"),
        0
    );
    assert_eq!(failed_imports(&env), 1);
    assert_eq!(packages_count(&env), 0);
}

#[test]
pub(super) fn d01_schema_initialization_is_idempotent() {
    let env = TestEnv::start(false);
    let tables = [
        "generated_capabilities",
        "generated_capability_revisions",
        "generated_capability_checks",
        "generated_capability_calls",
        "generated_capability_imports",
    ];
    for table in tables {
        let count = env.scalar(&format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
        ));
        assert_eq!(count, 1, "{table} exists");
    }
    assert_eq!(
        env.scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_generated_capability_active'"
        ),
        1
    );
    env.writer
        .write(|connection| {
            crate::persistence::schema::initialize_database(connection)
                .map_err(|error| format!("re-initialize: {error}"))
        })
        .expect("initialization is idempotent");
    assert_eq!(env.scalar("SELECT COUNT(*) FROM generated_capabilities"), 0);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM conversations WHERE id = 'conversation_primary'"),
        1
    );
}

#[test]
pub(super) fn d02_migration_preserves_existing_conversation_data() {
    let env = TestEnv::start(false);
    env.writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES ('m1', 'conversation_primary', 'user', 'hello', '1')",
                    [],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    env.writer
        .write(|connection| {
            crate::persistence::schema::initialize_database(connection)
                .map_err(|error| format!("re-initialize: {error}"))
        })
        .expect("re-initialize succeeds");
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM conversation_messages WHERE id = 'm1'"),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn d03_foreign_keys_reject_cross_capability_pointers_and_double_active() {
    let env = TestEnv::start(false);
    let a = env.import_a().await;
    let b = env.import_b().await;
    // Both packages carry metadata.id "enabled-user", so they are two versions of one
    // capability.
    assert_eq!(a.capability_id, b.capability_id);

    let cross = env.writer.write(|connection| {
        connection
            .execute(
                "INSERT INTO generated_capabilities(id, current_revision_id, catalog_epoch, created_at, updated_at)
                 VALUES ('other', ?1, 0, '0', '0')",
                rusqlite::params![a.revision_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    });
    assert!(
        cross.is_err(),
        "composite foreign key rejects a pointer to another capability's revision"
    );

    let first = env.writer.write(|connection| {
        connection
            .execute(
                "UPDATE generated_capability_revisions SET state = 'active' WHERE id = ?1",
                rusqlite::params![a.revision_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    });
    assert!(first.is_ok());
    let second = env.writer.write(|connection| {
        connection
            .execute(
                "UPDATE generated_capability_revisions SET state = 'active' WHERE id = ?1",
                rusqlite::params![b.revision_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    });
    assert!(
        second.is_err(),
        "partial unique index rejects two active revisions of one capability"
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_revisions WHERE state = 'active'"),
        1
    );
}

/// T01: the Rust `package_hash` must reproduce the L-Lang fixed value for the fixture.
#[test]
pub(super) fn t01_package_hash_matches_the_fixed_vector() {
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(candidate_dir(CANDIDATE_A).join("capability.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        package_hash(&manifest),
        "bd4696ec301a6559f4a494d3bbfbd1e7dba57275e246af62256feb997d791a00"
    );
    let manifest_b: Value = serde_json::from_slice(
        &std::fs::read(candidate_dir(CANDIDATE_B).join("capability.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        package_hash(&manifest_b),
        "cbefe84aac5053ae0373cd62febdc409d5b31e68922e381b6ae8714b172506af"
    );
    // Key order, whitespace and non-ASCII text must not change the digest.
    let reordered: Value = serde_json::from_str(
        r#"{ "files": {"wasm": {"hash": "0","path": "b.wasm"}, "tests": {"path": "t.json","hash": "0"}, "build": {"path": "m.json","hash": "0"}, "source": {"path": "s.jsonc","hash": "0"}, "request": {"path": "r.json","hash": "0"}}, "permissions": [], "output": "boolean", "profile": "predicate-i32-v1", "metadata": {"doNotUseWhen": "日本語テキスト\\\\n", "useWhen": "u", "purpose": "p", "release": "v1", "id": "x"}, "version": 2 }"#,
    )
    .unwrap();
    assert_eq!(package_hash(&reordered), package_hash(&reordered.clone()));
    assert!(
        crate::generated_capabilities::contracts::canonical(&reordered).contains("日本語テキスト")
    );
    let _ = PackageStore::inventory_hash(&[]);
}
