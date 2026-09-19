//! Shared real-runtime helper for cancellation tests: a trusted bundle whose host CLI
//! announces its PID and then blocks until it is cancelled.

use super::*;
use crate::generated_capabilities::contracts::sha256_hex;

pub(crate) fn install(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let root = base.join("runtime");
    copy_tree(&fixture_root().join("runtime"), &root);
    let gate = base.join("hang");
    let marker = base.join("started.json");
    let cli = root.join("capability-host-cli.ts");
    let source = fs::read_to_string(&cli).unwrap();
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
    (root, gate, marker)
}

/// Waits until the hanging child has written its PID, returning it.
pub(crate) async fn await_child(marker: &Path) -> u32 {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = fs::read(marker) {
                if let Ok(pid) = serde_json::from_slice(&bytes) {
                    break pid;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("real host process must announce startup")
}

/// Asserts the cancelled child was reaped. Bun's signal-0 check is portable across the same
/// platforms as the runtime.
pub(crate) fn assert_reaped(pid: u32) {
    let status = std::process::Command::new(bun_path())
        .args([
            "-e",
            &format!(
                "try {{ process.kill({pid}, 0); process.exit(1); }} catch (e) {{ process.exit(e.code === 'ESRCH' ? 0 : 2); }}"
            ),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "cancelled child must have been reaped");
}

/// Waits until every call is terminal, no execution is registered, and the process slot is free.
pub(crate) async fn await_settled(env: &TestEnv) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let running = env
                .scalar("SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'running'");
            if running == 0 && env.service.active_executions() == 0 {
                if let Ok(permit) = env.service.occupy_process_slot() {
                    drop(permit);
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled execution must settle and release capacity");
}
