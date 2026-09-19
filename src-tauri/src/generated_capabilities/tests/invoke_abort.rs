use super::*;
use crate::generated_capabilities::{contracts::sha256_hex, service::InvokeRequest};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s02_abort_cancels_started_process_and_releases_capacity() {
    let mut env = TestEnv::start(true);
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("runtime");
    copy_tree(&fixture_root().join("runtime"), &root);
    let gate = temporary.path().join("hang");
    let marker = temporary.path().join("started.json");
    let cli = root.join("capability-host-cli.ts");
    let source = fs::read_to_string(&cli).unwrap();
    // A real child announces its PID, then cannot finish until cancelled. The gate is
    // enabled only after verification, so setup uses the ordinary trusted runtime.
    let prefix = format!(
        "if (await Bun.file({gate}).exists()) {{ await Bun.write({marker}, JSON.stringify(process.pid)); setInterval(() => {{}}, 1000); await new Promise(() => {{}}); }}\n",
        gate = serde_json::to_string(&gate).unwrap(),
        marker = serde_json::to_string(&marker).unwrap(),
    );
    fs::write(&cli, format!("{prefix}{source}")).unwrap();
    let manifest_path = root.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["files"]["capability-host-cli.ts"] = json!(sha256_hex(&fs::read(&cli).unwrap()));
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    env.service = super::runtime_change::rebuild_service(&env, &root);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env.service.resolve_active(&revision.capability_id).unwrap();
    fs::write(&gate, b"hang").unwrap();

    let service = env.service.clone();
    let call = tokio::spawn(async move {
        service
            .invoke(
                InvokeRequest::new(
                    resolved,
                    "aborted".into(),
                    object(json!({"enabled": true, "suspended": false})),
                ),
                &Cancellation::default(),
            )
            .await
    });
    let pid: u32 = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = fs::read(&marker) {
                if let Ok(pid) = serde_json::from_slice(&bytes) {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("real host process must announce startup");
    assert_eq!(env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE id='aborted' AND status='running'"), 1);
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let cancelled = env.scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE id='aborted' AND status='cancelled' AND error_code='cancelled'");
            if cancelled == 1 && env.service.active_executions() == 0 {
                if let Ok(permit) = env.service.occupy_process_slot() {
                    drop(permit);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("abort must cancel and settle before the host timeout");
    // Bun's signal-0 check is portable across the same platforms as the runtime.
    let status = std::process::Command::new(bun_path()).args([
        "-e", &format!("try {{ process.kill({pid}, 0); process.exit(1); }} catch (e) {{ process.exit(e.code === 'ESRCH' ? 0 : 2); }}")
    ]).status().unwrap();
    assert!(status.success(), "cancelled child must have been reaped");
    fs::remove_file(gate).unwrap();
    let result = env
        .service
        .invoke(
            InvokeRequest::new(
                env.service.resolve_active(&revision.capability_id).unwrap(),
                "after-abort".into(),
                object(json!({"enabled": true, "suspended": false})),
            ),
            &Cancellation::default(),
        )
        .await
        .unwrap();
    assert!(
        result.value,
        "the next real invocation can use the released slot"
    );
    assert!(env.service.shutdown());
}
