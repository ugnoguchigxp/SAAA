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
use super::copy_tree;
use crate::persistence::SqliteWriter;
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};
pub(crate) const CANDIDATE_A: &str = "candidate-a";
pub(crate) const CANDIDATE_B: &str = "candidate-b";
pub(crate) const ACCEPTANCE_A: &str = "enabled-user-truth-table";
pub(crate) const ACCEPTANCE_B: &str = "enabled-only-truth-table";
pub(crate) fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/llang-capability-v2")
}
pub(crate) fn candidate_dir(name: &str) -> PathBuf {
    fixture_root().join(name)
}
pub(crate) fn bun_path() -> PathBuf {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .map(|directory| directory.join(if cfg!(windows) { "bun.exe" } else { "bun" }))
        .find(|path| path.is_file())
        .and_then(|path| path.canonicalize().ok())
        .expect("Bun must be available on PATH for fixture tests")
}
pub(crate) fn runtime_files() -> BTreeMap<String, String> {
    let bytes = fs::read(fixture_root().join("runtime/manifest.json")).expect("runtime manifest");
    let manifest: Value = serde_json::from_slice(&bytes).expect("runtime manifest is JSON");
    serde_json::from_value(manifest["files"].clone()).expect("runtime file map")
}
pub(crate) fn runtime_digest() -> String {
    runtime_bundle::runtime_digest(&runtime_files())
}
/// `SqliteWriter::open` is only allowed in `lib.rs`; tests use an in-memory migrated database.
pub(crate) fn test_writer() -> Arc<SqliteWriter> {
    let connection = rusqlite::Connection::open_in_memory().expect("in-memory database");
    crate::persistence::schema::initialize_database(&connection).expect("schema initializes");
    Arc::new(SqliteWriter::from_connection(connection))
}
/// A temporary SAAA data directory with the generated-capability catalog initialized.
pub(crate) struct TestEnv {
    pub(crate) _directory: tempfile::TempDir,
    pub(crate) data_directory: PathBuf,
    pub(crate) writer: Arc<SqliteWriter>,
    pub(crate) service: Arc<CapabilityService>,
    pub(crate) runtime_digest: String,
}
impl TestEnv {
    /// `exposure` gates activate/invoke; import/verify always need a trusted runtime.
    pub(crate) fn start(exposure: bool) -> Self {
        Self::start_with(exposure, None)
    }

    pub(crate) fn start_with(exposure: bool, runtime_root: Option<PathBuf>) -> Self {
        Self::start_full(exposure, runtime_root, fixture_root().join("acceptance"))
    }

    /// A service whose acceptance ledger is a test-owned directory instead of the fixture.
    pub(crate) fn start_with_ledger(exposure: bool, ledger_directory: PathBuf) -> Self {
        Self::start_full(exposure, None, ledger_directory)
    }

    fn start_full(
        exposure: bool,
        runtime_root: Option<PathBuf>,
        ledger_directory: PathBuf,
    ) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let data_directory = directory.path().to_path_buf();
        let writer = test_writer();
        let runtime_root = runtime_root.unwrap_or_else(|| fixture_root().join("runtime"));
        let config_path = data_directory.join("runtime-config.json");
        let digest = runtime_digest();
        fs::write(
            &config_path,
            json!({
                "formatVersion": 1,
                "enabled": exposure,
                "bunPath": bun_path().to_string_lossy(),
                "runtimeRoot": runtime_root.to_string_lossy(),
                "expectedRuntimeDigest": digest,
            })
            .to_string(),
        )
        .expect("runtime config written");
        let config = runtime_bundle::load_config(&config_path).expect("runtime config loads");
        let service = Arc::new(CapabilityService::build(
            writer.clone(),
            &data_directory,
            ledger_directory,
            Some(config),
        ));
        service.store().ensure_layout();
        Self {
            _directory: directory,
            data_directory,
            writer,
            service,
            runtime_digest: digest,
        }
    }

    /// A service without runtime configuration: everything is disabled.
    pub(crate) fn start_disabled() -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let data_directory = directory.path().to_path_buf();
        let writer = test_writer();
        let service = Arc::new(CapabilityService::build(
            writer.clone(),
            &data_directory,
            fixture_root().join("acceptance"),
            None,
        ));
        service.store().ensure_layout();
        Self {
            _directory: directory,
            data_directory,
            writer,
            service,
            runtime_digest: String::new(),
        }
    }

    pub(crate) fn host(&self) -> Arc<WasmHost> {
        let config_path = self.data_directory.join("runtime-config.json");
        let config = runtime_bundle::load_config(&config_path).expect("runtime config loads");
        let runtime = runtime_bundle::validate(&config).expect("runtime validates");
        Arc::new(WasmHost::new(runtime, config.bun_path).expect("host builds"))
    }

    pub(crate) fn candidate(&self, name: &str, acceptance_id: &str) -> ImportCandidate {
        ImportCandidate {
            capability_id: None,
            candidate_directory: candidate_dir(name),
            acceptance_id: acceptance_id.to_string(),
            provenance: "fixture".to_string(),
        }
    }

    pub(crate) async fn import(
        &self,
        name: &str,
        acceptance_id: &str,
    ) -> CapabilityResult<RevisionRef> {
        self.service
            .import_candidate(&self.candidate(name, acceptance_id))
            .await
    }

    pub(crate) async fn import_a(&self) -> RevisionRef {
        self.import(CANDIDATE_A, ACCEPTANCE_A)
            .await
            .expect("candidate A imports")
    }

    pub(crate) async fn import_b(&self) -> RevisionRef {
        self.import(CANDIDATE_B, ACCEPTANCE_B)
            .await
            .expect("candidate B imports")
    }

    pub(crate) async fn verify(
        &self,
        revision: &RevisionRef,
        acceptance_id: &str,
    ) -> CapabilityResult<super::super::service::VerificationSummary> {
        self.service
            .verify_candidate(
                &revision.revision_id,
                acceptance_id,
                &Cancellation::default(),
            )
            .await
    }

    /// Imports, verifies and activates a candidate, returning the active revision.
    pub(crate) async fn ready(&self, name: &str, acceptance_id: &str) -> RevisionRef {
        let revision = self
            .import(name, acceptance_id)
            .await
            .expect("candidate imports");
        let report = self
            .verify(&revision, acceptance_id)
            .await
            .expect("verification runs");
        assert!(report.passed, "candidate must verify: {}", report.detail);
        let epoch = self.service.catalog_epoch(&revision.capability_id).unwrap();
        self.service
            .activate_revision(&revision.revision_id, epoch)
            .expect("revision activates");
        revision
    }

    pub(crate) fn revision_state(&self, revision_id: &str) -> RevisionState {
        self.service
            .read_revision(revision_id)
            .expect("revision exists")
            .state
    }

    pub(crate) fn call_count(&self, revision_id: &str) -> i64 {
        self.writer
            .read_serialized(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM generated_capability_calls WHERE revision_id = ?1",
                        rusqlite::params![revision_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(crate::database_error)
            })
            .expect("call count")
    }

    pub(crate) fn scalar(&self, sql: &str) -> i64 {
        self.writer
            .read_serialized(|connection| {
                connection
                    .query_row(sql, [], |row| row.get::<_, i64>(0))
                    .map_err(crate::database_error)
            })
            .expect("scalar query")
    }
}
pub(crate) fn fixture_host(env: &TestEnv) -> Arc<WasmHost> {
    env.host()
}
pub(crate) fn host_request_from(value: &Value) -> HostRequest {
    let request_id = value["requestId"].as_str().expect("requestId");
    let package_hash = value["packageHash"].as_str().expect("packageHash");
    match value["operation"].as_str().expect("operation") {
        "inspect" => HostRequest::inspect(request_id, package_hash),
        "verify" => HostRequest::verify(request_id, package_hash),
        "invoke" => HostRequest::invoke(
            request_id,
            package_hash,
            value["input"].as_object().expect("input").clone(),
            value["timeoutMs"].as_u64().expect("timeoutMs"),
        ),
        other => panic!("unexpected operation {other}"),
    }
}
pub(crate) fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object")
}
pub(crate) fn wire(name: &str) -> (HostRequest, Value) {
    let bytes = fs::read(fixture_root().join("wire").join(name)).expect("wire fixture");
    let value: Value = serde_json::from_slice(&bytes).expect("wire fixture is JSON");
    (
        host_request_from(&value["request"]),
        value["response"].clone(),
    )
}
pub(crate) fn parse_wire(name: &str) -> HostResponse {
    let (request, response) = wire(name);
    parse_response(response, &request).expect("wire fixture parses")
}
pub(crate) fn script_command(root: &Path, name: &str, source: &str) -> RuntimeCommand {
    let script = root.join(format!("{name}.ts"));
    fs::write(&script, source).expect("script written");
    RuntimeCommand {
        executable: bun_path(),
        arguments: vec![script.canonicalize().expect("script path")],
        current_dir: root.canonicalize().expect("script directory"),
    }
}
pub(crate) fn revision_row(env: &TestEnv, revision_id: &str) -> repository::RevisionRow {
    env.service
        .read_revision(revision_id)
        .expect("revision row")
}
// ---------------------------------------------------------------------------
// W01-W05: strict v2 wire parsing
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn w01_real_v2_responses_parse_strictly() {
    let inspect = parse_wire("inspect-a.json");
    assert!(matches!(
        inspect.outcome,
        HostOutcome::Ok(OperationResult::Inspect(_))
    ));
    let verify = parse_wire("verify-a.json");
    match verify.outcome {
        HostOutcome::Ok(OperationResult::Verify(report)) => {
            assert_eq!(report.status, ReportStatus::Pass);
            assert_eq!(report.verifier, "llang-capability-v2");
        }
        other => panic!("unexpected verify outcome: {other:?}"),
    }
    let invoke = parse_wire("invoke-true.json");
    assert!(matches!(
        invoke.outcome,
        HostOutcome::Ok(OperationResult::Invoke(true))
    ));
    let invalid = parse_wire("invoke-invalid-type.json");
    assert!(matches!(invalid.outcome, HostOutcome::Error(_)));
}
#[test]
pub(crate) fn w02_protocol_id_and_hash_correlation_is_enforced() {
    for (name, pointer, value) in [
        ("inspect-a.json", "/protocol", json!("llang-host-v0")),
        ("inspect-a.json", "/requestId", json!("other-request")),
        ("inspect-a.json", "/packageHash", json!("0".repeat(64))),
    ] {
        let (request, mut response) = wire(name);
        *response.pointer_mut(pointer).unwrap() = value;
        assert!(parse_response(response, &request).is_err(), "{pointer}");
    }
    // An error response must not be accepted when it names another package.
    let (request, mut response) = wire("invoke-invalid-type.json");
    *response.pointer_mut("/packageHash").unwrap() = json!("0".repeat(64));
    assert!(
        parse_response(response, &request).is_err(),
        "error packageHash"
    );
}
#[test]
pub(crate) fn w03_string_boolean_is_not_a_success_value() {
    let (request, mut response) = wire("invoke-true.json");
    *response.pointer_mut("/result/value").unwrap() = json!("false");
    assert!(parse_response(response, &request).is_err());
}
#[test]
pub(crate) fn w04_outer_ok_does_not_hide_inner_failures() {
    let (request, response) = wire("verify-a.json");
    let mut failed = response.clone();
    *failed
        .pointer_mut("/result/results/0/actual/value")
        .unwrap() = json!(false);
    *failed.pointer_mut("/result/results/0/status").unwrap() = json!("fail");
    *failed.pointer_mut("/result/status").unwrap() = json!("fail");
    let parsed = parse_response(failed, &request).expect("structurally valid report");
    match parsed.outcome {
        HostOutcome::Ok(OperationResult::Verify(report)) => {
            assert_eq!(report.status, ReportStatus::Fail);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    let mut contradictory = response.clone();
    *contradictory
        .pointer_mut("/result/results/0/status")
        .unwrap() = json!("fail");
    assert!(parse_response(contradictory, &request).is_err());

    let mut errored = response;
    *errored.pointer_mut("/result/diagnostics").unwrap() = json!(["synthetic"]);
    *errored.pointer_mut("/result/status").unwrap() = json!("error");
    let parsed = parse_response(errored, &request).expect("structurally valid error report");
    match parsed.outcome {
        HostOutcome::Ok(OperationResult::Verify(report)) => {
            assert_eq!(report.status, ReportStatus::Error);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}
#[test]
pub(crate) fn w05_unknown_fields_versions_and_empty_cases_are_rejected() {
    let (request, base) = wire("verify-a.json");

    let mut unknown_field = base.clone();
    unknown_field
        .as_object_mut()
        .unwrap()
        .insert("extra".into(), json!(1));
    assert!(parse_response(unknown_field, &request).is_err());

    let mut unknown_version = base.clone();
    *unknown_version.pointer_mut("/result/version").unwrap() = json!(3);
    assert!(parse_response(unknown_version, &request).is_err());

    let mut unknown_verifier = base.clone();
    *unknown_verifier.pointer_mut("/result/verifier").unwrap() = json!("capability-predicate-v1");
    assert!(parse_response(unknown_verifier, &request).is_err());

    let mut empty = base;
    *empty.pointer_mut("/result/results").unwrap() = json!([]);
    assert!(parse_response(empty, &request).is_err());
}
// ---------------------------------------------------------------------------
// H01-H04: host execution
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(crate) async fn h01_both_candidates_run_on_one_runtime_digest() {
    let env = TestEnv::start(false);
    let host = fixture_host(&env);
    let cancellation = Cancellation::default();
    let a = env.import_a().await;
    let b = env.import_b().await;
    assert_ne!(a.package_hash, b.package_hash);
    let digest_before = host.runtime_digest().to_string();

    for (package_hash, enabled, suspended, expected) in [
        (a.package_hash.as_str(), true, true, false),
        (a.package_hash.as_str(), true, false, true),
        (a.package_hash.as_str(), false, false, false),
        (b.package_hash.as_str(), true, true, true),
        (b.package_hash.as_str(), true, false, true),
        (b.package_hash.as_str(), false, true, false),
    ] {
        let manifest = env.service.store().manifest_path(package_hash);
        let response = host
            .execute(
                &HostRequest::invoke(
                    "h01",
                    package_hash,
                    object(json!({ "enabled": enabled, "suspended": suspended })),
                    5_000,
                ),
                &manifest,
                &cancellation,
            )
            .await
            .expect("invocation succeeds");
        assert!(matches!(
            response.outcome,
            HostOutcome::Ok(OperationResult::Invoke(value)) if value == expected
        ));
    }
    assert_eq!(host.runtime_digest(), digest_before.as_str());
    assert_eq!(host.runtime_digest(), env.runtime_digest);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(crate) async fn h02_tampered_runtime_and_missing_bun_fail_before_spawn() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime_root = temporary.path().join("runtime");
    copy_tree(&fixture_root().join("runtime"), &runtime_root);
    fs::write(runtime_root.join("capability-host-cli.ts"), "tampered").unwrap();

    let env = TestEnv::start_with(false, Some(runtime_root));
    assert!(!env.service.is_ready());
    let error = env
        .import(CANDIDATE_A, ACCEPTANCE_A)
        .await
        .expect_err("tampered runtime is rejected");
    assert_eq!(error.code, CapabilityErrorCode::Unavailable);

    let missing_bun = TestEnv::start(false);
    let config =
        runtime_bundle::load_config(&missing_bun.data_directory.join("runtime-config.json"))
            .unwrap();
    let validation = runtime_bundle::validate(&config).unwrap();
    let error = WasmHost::new(validation, PathBuf::from("/definitely/missing/bun"))
        .expect_err("missing Bun fails");
    assert_eq!(error.code, CapabilityErrorCode::Unavailable);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(crate) async fn h03_hanging_host_is_killed_reaped_and_replaced() {
    let env = TestEnv::start(false);
    let host = fixture_host(&env);
    let temporary = tempfile::tempdir().unwrap();
    let command = script_command(
        temporary.path(),
        "hang",
        "await Bun.stdin.text(); setInterval(() => {}, 1000);",
    );
    let request = HostRequest::inspect("h03-hang", &"0".repeat(64));
    let cancellation = Cancellation::default();

    let mut short = TestEnv::start(false).host();
    Arc::get_mut(&mut short).unwrap().timeouts.inspect = std::time::Duration::from_millis(200);
    let error = short
        .execute_with_command(&request, temporary.path(), &cancellation, command.clone())
        .await
        .expect_err("hang times out");
    assert_eq!(error.code, CapabilityErrorCode::Timeout);

    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let error = host
        .execute_with_command(&request, temporary.path(), &cancellation, command)
        .await
        .expect_err("hang is cancelled");
    assert_eq!(error.code, CapabilityErrorCode::Cancelled);

    let ok = host
        .execute(
            &HostRequest::inspect("h03-ok", &"0".repeat(64)),
            &fixture_root().join("runtime"),
            &Cancellation::default(),
        )
        .await
        .expect("host responds after recovery");
    assert!(matches!(ok.outcome, HostOutcome::Error(_)));

    let overflow = script_command(
        temporary.path(),
        "overflow",
        "await Bun.stdin.text(); process.stdout.write('x'.repeat(1024 * 1024 + 1));",
    );
    let error = host
        .execute_with_command(
            &HostRequest::inspect("h03-overflow", &"0".repeat(64)),
            temporary.path(),
            &Cancellation::default(),
            overflow,
        )
        .await
        .expect_err("output limit fails");
    assert_eq!(error.code, CapabilityErrorCode::OutputLimit);
}
