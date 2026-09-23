//! Regression coverage for a runtime that changes after the service trusted it.

use super::*;
use crate::generated_capabilities::contracts::sha256_hex;
use crate::generated_capabilities::service::CapabilityService;

/// Digest of an arbitrary runtime root, read from its own manifest.
pub(crate) fn runtime_digest_for(root: &Path) -> String {
    let bytes = fs::read(root.join("manifest.json")).expect("runtime manifest");
    let manifest: Value = serde_json::from_slice(&bytes).expect("runtime manifest is JSON");
    let files: BTreeMap<String, String> =
        serde_json::from_value(manifest["files"].clone()).expect("runtime file map");
    runtime_bundle::runtime_digest(&files)
}

/// Copies the fixture runtime into a consistent but *different* bundle: one worker is rewritten
/// and the manifest records the new hash, so the copy is trusted yet has a new digest.
pub(crate) fn modified_runtime(destination: &Path) -> String {
    copy_tree(&fixture_root().join("runtime"), destination);
    let worker = destination.join("capability-worker.ts");
    let mut source = fs::read_to_string(&worker).expect("worker reads");
    source.push_str("\n// modified for the runtime-change regression test\n");
    fs::write(&worker, source).expect("worker writes");

    let manifest_path = destination.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).expect("manifest is JSON");
    let names = manifest["files"]
        .as_object()
        .expect("files object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut files = BTreeMap::new();
    for name in names {
        let bytes = fs::read(destination.join(&name)).expect("runtime file reads");
        files.insert(name, sha256_hex(&bytes));
    }
    manifest["files"] = serde_json::to_value(&files).expect("files serialise");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest serialises"),
    )
    .expect("manifest writes");
    runtime_bundle::runtime_digest(&files)
}

/// A second service over the same catalog but a different trusted runtime.
pub(crate) fn rebuild_service(env: &TestEnv, runtime_root: &Path) -> Arc<CapabilityService> {
    let config_path = env.data_directory.join("runtime-config-changed.json");
    let digest = runtime_digest_for(runtime_root);
    fs::write(
        &config_path,
        json!({
            "formatVersion": 1,
            "enabled": true,
            "bunPath": bun_path().to_string_lossy(),
            "runtimeRoot": runtime_root.to_string_lossy(),
            "expectedRuntimeDigest": digest,
        })
        .to_string(),
    )
    .expect("runtime config written");
    let config = runtime_bundle::load_config(&config_path).expect("runtime config loads");
    Arc::new(CapabilityService::build(
        env.writer.clone(),
        &env.data_directory,
        fixture_root().join("acceptance"),
        Some(config),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn h05_runtime_modified_after_construction_is_never_executed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("runtime");
    copy_tree(&fixture_root().join("runtime"), &root);
    let env = TestEnv::start_with(false, Some(root.clone()));
    assert!(env.service.is_ready());
    let host = fixture_host(&env);
    let revision = env.import_a().await;
    let manifest_path = env.service.store().manifest_path(&revision.package_hash);
    assert!(host
        .execute(
            &HostRequest::inspect("h05-before", &revision.package_hash),
            &manifest_path,
            &Cancellation::default(),
        )
        .await
        .is_ok());

    // Replace the host CLI after the service was constructed and trusted.
    fs::write(
        root.join("capability-host-cli.ts"),
        "const request = JSON.parse(await Bun.stdin.text()); process.stdout.write(JSON.stringify({protocol: 'llang-host-v1', requestId: request.requestId, packageHash: request.packageHash, elapsedMs: 0, apiCalls: 0, status: 'ok', result: { value: false }}));",
    )
    .expect("tampered runtime written");

    let error = host
        .execute(
            &HostRequest::inspect("h05-after", &revision.package_hash),
            &manifest_path,
            &Cancellation::default(),
        )
        .await
        .expect_err("a runtime changed after trust must not run");
    assert_eq!(error.code, CapabilityErrorCode::Unavailable);
}
