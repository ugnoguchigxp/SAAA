use super::test_env::*;
use super::super::{
    contracts::{
        parse_response, HostOutcome, HostRequest, HostResponse, OperationResult, ReportStatus,
    },
    errors::*,
    host::{
        process::{Cancellation, RuntimeCommand},
        runtime_bundle, WasmHost,
    },
    repository::{self, RevisionState},
    service::{CapabilityService, ImportCandidate, RevisionRef},
};
use crate::persistence::SqliteWriter;
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn h04_child_environment_is_cleared() {
    let temporary = tempfile::tempdir().unwrap();
    let command = script_command(
        temporary.path(),
        "env",
        "await Bun.stdin.text(); process.stderr.write(String(process.env.SAAA_TEST_CREDENTIAL ?? 'absent')); process.exit(7);",
    );
    // The credential is present in the parent process but must not reach the child.
    std::env::set_var("SAAA_TEST_CREDENTIAL", "secret-value");
    let error = super::super::host::process::execute(
        &command,
        b"{}",
        std::time::Duration::from_secs(5),
        &Cancellation::default(),
    )
    .await
    .expect_err("child exits unsuccessfully");
    std::env::remove_var("SAAA_TEST_CREDENTIAL");
    assert_eq!(error.exit_code, Some(7));
    assert_eq!(error.stderr, "absent");
    assert!(!error.stderr.contains("secret-value"));
}
pub(crate) fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn feature_flag_off_refuses_management_and_execution() {
    let env = TestEnv::start_disabled();
    assert!(!env.service.is_ready());
    assert!(!env.service.is_exposed());

    let error = env
        .import(CANDIDATE_A, ACCEPTANCE_A)
        .await
        .expect_err("import is disabled without a runtime");
    assert_eq!(error.code, CapabilityErrorCode::Disabled);
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_revisions"),
        0
    );
    assert_eq!(
        env.scalar("SELECT COUNT(*) FROM generated_capability_imports"),
        0
    );
    assert_eq!(
        env.service
            .resolve_active("cap_missing")
            .expect_err("unknown capability")
            .code,
        CapabilityErrorCode::Conflict
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn m0r_runtime_is_reusable_from_a_moved_directory_with_spaces() {
    let temporary = tempfile::tempdir().unwrap();
    let moved = temporary.path().join("runtime with spaces");
    copy_tree(&fixture_root().join("runtime"), &moved);

    let env = TestEnv::start_with(false, Some(moved));
    assert!(
        env.service.is_ready(),
        "the moved runtime is trusted by digest"
    );
    let host = fixture_host(&env);
    assert_eq!(host.runtime_digest(), env.runtime_digest);
    let revision = env.import_a().await;
    let response = host
        .execute(
            &HostRequest::inspect("m0r-inspect", &revision.package_hash),
            &env.service.store().manifest_path(&revision.package_hash),
            &Cancellation::default(),
        )
        .await
        .expect("the moved runtime inspects the candidate");
    assert!(matches!(
        response.outcome,
        HostOutcome::Ok(OperationResult::Inspect(_))
    ));
}
