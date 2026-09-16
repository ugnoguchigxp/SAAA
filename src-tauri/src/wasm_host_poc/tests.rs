use super::{
    contracts::{
        parse_response, HostErrorCode, HostOutcome, HostRequest, OperationResult, ReportStatus,
    },
    kit::TrustedKit,
    process::{Cancellation, RuntimeCommand, TransportErrorKind},
    HostFailure, WasmHost,
};
use serde_json::{json, Map, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

const PACKAGE_HASH: &str = "1d31a73c93cb0136ef9b8521112a2c8efd42d79041d5008585b8874ffe3beb68";
const KIT_JSON_HASH: &str = "c3272ba0ff6331179b5e4a92d1b4f077d96c3bfabee7dadaab86fcd2d0f47575";
const LLANG_COMMIT: &str = "0124d704f8228cb0c34d6379cf0302641199d82e";
const BUN_VERSION: &str = "1.3.14";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_kit_inspect_verify_and_vectors_run_without_credentials() {
    let bun = bun_path();
    let version = std::process::Command::new(&bun)
        .arg("--version")
        .env_clear()
        .output()
        .expect("configured Bun version is readable");
    assert!(version.status.success());
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), BUN_VERSION);
    let host = host(fixture_root());
    let cancellation = Cancellation::default();

    let inspect = host
        .execute(
            &HostRequest::inspect("inspect-1", PACKAGE_HASH),
            &cancellation,
        )
        .await
        .expect("inspect succeeds");
    assert_eq!(inspect.response.request_id, "inspect-1");
    assert_eq!(inspect.response.package_hash.as_deref(), Some(PACKAGE_HASH));
    assert!(inspect.response.elapsed_ms < 15_000);
    assert!(matches!(
        inspect.response.outcome,
        HostOutcome::Ok(OperationResult::Inspect(_))
    ));
    assert!(inspect.stderr.is_empty());
    assert_eq!(inspect.llang_commit, LLANG_COMMIT);
    assert_eq!(inspect.bun_version, BUN_VERSION);

    let verify = host
        .execute(
            &HostRequest::verify("verify-1", PACKAGE_HASH),
            &cancellation,
        )
        .await
        .expect("verify succeeds");
    match verify.response.outcome {
        HostOutcome::Ok(OperationResult::Verify(report)) => {
            assert_eq!(report.status, ReportStatus::Pass);
            assert_eq!(report.package_hash.as_deref(), Some(PACKAGE_HASH));
        }
        other => panic!("unexpected verify result: {other:?}"),
    }

    for (index, enabled, suspended, expected) in [
        (0, true, true, false),
        (1, true, false, true),
        (2, false, true, false),
        (3, false, false, false),
    ] {
        let request = HostRequest::invoke(
            &format!("access-{index}"),
            PACKAGE_HASH,
            object(json!({"enabled": enabled, "suspended": suspended})),
            Vec::new(),
            10_000,
        );
        let record = host
            .execute(&request, &cancellation)
            .await
            .expect("fixture invocation succeeds");
        assert!(matches!(
            record.response.outcome,
            HostOutcome::Ok(OperationResult::Invoke(value)) if value == expected
        ));
    }

    let invalid = HostRequest::invoke(
        "bad-input",
        PACKAGE_HASH,
        object(json!({"enabled": "true", "suspended": false})),
        Vec::new(),
        10_000,
    );
    assert!(matches!(
        host.execute(&invalid, &cancellation)
            .await
            .expect("structured invalid-input response")
            .response
            .outcome,
        HostOutcome::Error(HostErrorCode::InvalidInput)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn package_mismatch_spaces_and_copied_candidate_tampering_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let copied = temporary.path().join("copied kit with spaces");
    copy_tree(&fixture_root(), &copied);
    let host = host(copied.clone());
    let cancellation = Cancellation::default();

    let mismatch = HostRequest::inspect("wrong-hash", &"0".repeat(64));
    assert!(matches!(
        host.execute(&mismatch, &cancellation)
            .await
            .expect("package mismatch is a structured response")
            .response
            .outcome,
        HostOutcome::Error(HostErrorCode::PackageMismatch)
    ));

    fs::OpenOptions::new()
        .append(true)
        .open(copied.join("candidate/capability.json"))
        .unwrap()
        .write_all(b" ")
        .unwrap();
    let failure = host
        .execute(
            &HostRequest::inspect("tampered", PACKAGE_HASH),
            &cancellation,
        )
        .await
        .expect_err("tampered kit must not run");
    assert!(matches!(failure, HostFailure::Kit(message) if message.contains("hash mismatch")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_schema_conflicts_and_size_limit_fail_before_spawn() {
    let host = host(fixture_root());
    let cancellation = Cancellation::default();
    let conflict = HostRequest::invoke(
        "conflict",
        PACKAGE_HASH,
        object(json!({"enabled": true})),
        vec!["enabled".into()],
        100,
    );
    assert!(matches!(
        host.execute(&conflict, &cancellation).await,
        Err(HostFailure::Request(message)) if message.contains("conflicts")
    ));

    let invalid_id = HostRequest::inspect("contains a space", PACKAGE_HASH);
    assert!(matches!(
        host.execute(&invalid_id, &cancellation).await,
        Err(HostFailure::Request(message)) if message.contains("Schema")
    ));

    let duplicate_undefined = HostRequest::invoke(
        "duplicate-undefined",
        PACKAGE_HASH,
        Map::new(),
        vec!["missing".into(), "missing".into()],
        100,
    );
    assert!(matches!(
        host.execute(&duplicate_undefined, &cancellation).await,
        Err(HostFailure::Request(message)) if message.contains("Schema")
    ));

    let oversized = HostRequest::invoke(
        "oversized",
        PACKAGE_HASH,
        object(json!({"payload": "x".repeat(70 * 1024)})),
        Vec::new(),
        100,
    );
    assert!(matches!(
        host.execute(&oversized, &cancellation).await,
        Err(HostFailure::Request(message)) if message.contains("64 KiB")
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_empty_multiple_and_excessive_outputs_are_transport_failures() {
    let temporary = tempfile::tempdir().unwrap();
    let host = host(fixture_root());
    let request = HostRequest::inspect("mock-1", PACKAGE_HASH);
    let cancellation = Cancellation::default();

    for (name, source, expected) in [
        (
            "empty",
            "await Bun.stdin.text();",
            TransportErrorKind::Protocol,
        ),
        (
            "multiple",
            "await Bun.stdin.text(); process.stdout.write('{}\\n{}\\n');",
            TransportErrorKind::Protocol,
        ),
        (
            "stdout-limit",
            "await Bun.stdin.text(); process.stdout.write('x'.repeat(1024 * 1024 + 1));",
            TransportErrorKind::StdoutLimit,
        ),
        (
            "stderr-limit",
            "await Bun.stdin.text(); process.stderr.write('x'.repeat(64 * 1024 + 1));",
            TransportErrorKind::StderrLimit,
        ),
        (
            "nonzero",
            "await Bun.stdin.text(); process.stderr.write('safe diagnostic'); process.exit(7);",
            TransportErrorKind::Exit,
        ),
    ] {
        let command = script_command(temporary.path(), name, source, &[]);
        let failure = host
            .execute_with_command(&request, &cancellation, Some(command))
            .await
            .expect_err(name);
        match failure {
            HostFailure::Transport(error) => {
                assert_eq!(error.kind, expected, "{name}: {error:?}");
                assert!(error.reaped, "{name}: child was not reaped");
                if name == "nonzero" {
                    assert_eq!(error.exit_code, Some(7));
                    assert_eq!(error.stderr, "safe diagnostic");
                }
            }
            other => panic!("{name}: unexpected failure {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_and_cancel_kill_and_reap_the_hanging_runtime() {
    let temporary = tempfile::tempdir().unwrap();
    let mut host = host(fixture_root());
    host.timeouts.inspect = Duration::from_millis(150);
    let request = HostRequest::inspect("hang", PACKAGE_HASH);
    let command = script_command(
        temporary.path(),
        "hang",
        "await Bun.stdin.text(); setInterval(() => {}, 1000);",
        &[],
    );
    let failure = host
        .execute_with_command(&request, &Cancellation::default(), Some(command.clone()))
        .await
        .expect_err("hang must time out");
    assert_reaped(failure, TransportErrorKind::Timeout);

    host.timeouts.inspect = Duration::from_secs(5);
    let cancellation = Cancellation::default();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let failure = host
        .execute_with_command(&request, &cancellation, Some(command))
        .await
        .expect_err("hang must be cancelled");
    assert_reaped(failure, TransportErrorKind::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_schema_correlation_and_operation_result_types_are_enforced() {
    let temporary = tempfile::tempdir().unwrap();
    let host = host(fixture_root());
    let cancellation = Cancellation::default();
    let inspect = HostRequest::inspect("correlation", PACKAGE_HASH);
    let base = json!({
        "protocol": "llang-host-v1",
        "requestId": "correlation",
        "packageHash": PACKAGE_HASH,
        "elapsedMs": 1,
        "apiCalls": 0,
        "status": "ok",
        "result": {}
    });
    for (name, response) in [
        (
            "unknown-protocol",
            with_pointer(&base, "/protocol", json!("v0")),
        ),
        (
            "wrong-id",
            with_pointer(&base, "/requestId", json!("other")),
        ),
        (
            "wrong-hash",
            with_pointer(&base, "/packageHash", json!("0".repeat(64))),
        ),
    ] {
        let command = response_command(temporary.path(), name, &response);
        assert!(host
            .execute_with_command(&inspect, &cancellation, Some(command))
            .await
            .is_err());
    }

    let invoke = HostRequest::invoke(
        "typed-result",
        PACKAGE_HASH,
        object(json!({"enabled": true, "suspended": false})),
        Vec::new(),
        1000,
    );
    let invalid_result = json!({
        "protocol": "llang-host-v1", "requestId": "typed-result",
        "packageHash": PACKAGE_HASH, "elapsedMs": 1, "apiCalls": 0,
        "status": "ok", "result": {"value": "false"}
    });
    let command = response_command(temporary.path(), "invalid-result", &invalid_result);
    assert!(matches!(
        host.execute_with_command(&invoke, &cancellation, Some(command)).await,
        Err(HostFailure::Response(message)) if message.contains("invoke result")
    ));
}

#[test]
fn verify_outer_ok_does_not_turn_fail_or_error_reports_into_pass() {
    let request = HostRequest::verify("verify-report", PACKAGE_HASH);
    let report: Value =
        serde_json::from_slice(&fs::read(fixture_root().join("verification.json")).unwrap())
            .unwrap();

    let mut failed = report.clone();
    *failed.pointer_mut("/results/0/actual/value").unwrap() = json!(false);
    *failed.pointer_mut("/results/0/status").unwrap() = json!("fail");
    *failed.pointer_mut("/passed").unwrap() = json!(5);
    *failed.pointer_mut("/failed").unwrap() = json!(1);
    *failed.pointer_mut("/status").unwrap() = json!("fail");
    assert_report_status(
        envelope("verify-report", failed),
        &request,
        ReportStatus::Fail,
    );

    let mut errored = report;
    *errored.pointer_mut("/diagnostics").unwrap() = json!(["synthetic failure"]);
    *errored.pointer_mut("/errors").unwrap() = json!(1);
    *errored.pointer_mut("/status").unwrap() = json!("error");
    assert_report_status(
        envelope("verify-report", errored),
        &request,
        ReportStatus::Error,
    );
}

#[test]
fn missing_bun_is_reported_explicitly() {
    let trusted = trusted_kit(fixture_root());
    let failure = WasmHost::new(PathBuf::from("/definitely/missing/bun"), trusted)
        .expect_err("missing Bun must fail");
    assert!(matches!(failure, HostFailure::Kit(message) if message.contains("Bun path")));
}

fn host(root: PathBuf) -> WasmHost {
    WasmHost::new(bun_path(), trusted_kit(root)).expect("PoC host initializes")
}

fn trusted_kit(directory: PathBuf) -> TrustedKit {
    TrustedKit {
        directory,
        kit_json_hash: KIT_JSON_HASH.into(),
        package_hash: PACKAGE_HASH.into(),
        provenance_commit: LLANG_COMMIT.into(),
        bun_version: BUN_VERSION.into(),
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/llang-host-kit")
        .canonicalize()
        .unwrap()
}

fn bun_path() -> PathBuf {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .map(|directory| directory.join(if cfg!(windows) { "bun.exe" } else { "bun" }))
        .find(|path| path.is_file())
        .and_then(|path| path.canonicalize().ok())
        .expect("Bun 1.3.14 must be available on PATH")
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object")
}

fn copy_tree(source: &Path, destination: &Path) {
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

fn script_command(root: &Path, name: &str, source: &str, extra: &[PathBuf]) -> RuntimeCommand {
    let script = root.join(format!("{name}.ts"));
    fs::write(&script, source).unwrap();
    let mut arguments = vec![script.canonicalize().unwrap()];
    arguments.extend_from_slice(extra);
    RuntimeCommand {
        executable: bun_path(),
        arguments,
        current_dir: root.canonicalize().unwrap(),
    }
}

fn response_command(root: &Path, name: &str, response: &Value) -> RuntimeCommand {
    let response_path = root.join(format!("{name}.json"));
    fs::write(&response_path, serde_json::to_vec(response).unwrap()).unwrap();
    script_command(
        root,
        &format!("emit-{name}"),
        "await Bun.stdin.text(); process.stdout.write(await Bun.file(process.argv[2]).text());",
        &[response_path.canonicalize().unwrap()],
    )
}

fn with_pointer(base: &Value, pointer: &str, value: Value) -> Value {
    let mut result = base.clone();
    *result.pointer_mut(pointer).unwrap() = value;
    result
}

fn envelope(request_id: &str, result: Value) -> Value {
    json!({
        "protocol": "llang-host-v1", "requestId": request_id,
        "packageHash": PACKAGE_HASH, "elapsedMs": 1, "apiCalls": 0,
        "status": "ok", "result": result
    })
}

fn assert_report_status(value: Value, request: &HostRequest, expected: ReportStatus) {
    let response = parse_response(value, request).expect("report is structurally valid");
    match response.outcome {
        HostOutcome::Ok(OperationResult::Verify(report)) => assert_eq!(report.status, expected),
        other => panic!("unexpected response: {other:?}"),
    }
}

fn assert_reaped(failure: HostFailure, kind: TransportErrorKind) {
    match failure {
        HostFailure::Transport(error) => {
            assert_eq!(error.kind, kind);
            assert!(error.child_pid.is_some());
            assert!(error.reaped, "child was not reaped: {error:?}");
            assert!(!error.message.is_empty());
        }
        other => panic!("unexpected failure: {other:?}"),
    }
}

use std::io::Write;
