//! Executes only the user's saved argv recipes; no shell construction or inferred commands.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
#[cfg(unix)]
fn capture<R: Read + std::os::fd::AsRawFd + Send + 'static>(
    mut input: R,
    done: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Value> {
    std::thread::spawn(move || {
        let fd = input.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let mut bytes = Vec::new();
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut chunk = [0u8; 4096];
        let mut drained = true;
        let mut stop = None;
        loop {
            if done.load(Ordering::Acquire) {
                stop.get_or_insert_with(Instant::now);
            }
            if stop.is_some_and(|at: Instant| at.elapsed() > Duration::from_millis(500)) {
                drained = false;
                break;
            }
            match input.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    hash.update(&chunk[..n]);
                    total += n as u64;
                    let left = 16000 - bytes.len();
                    bytes.extend_from_slice(&chunk[..n.min(left)]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    drained = false;
                    break;
                }
            }
        }
        json!({"text":String::from_utf8_lossy(&bytes),"sha256":format!("{:x}",hash.finalize()),"bytes":total,"truncated":total>16000,"streamIncomplete":!drained})
    })
}
pub(super) fn check(
    workspace: &Path,
    args: &[String],
    seconds: u64,
    cancel: &dyn Fn() -> bool,
) -> Result<Value, String> {
    let mut command = saaa_terminal_agent_runtime::cli_command(Path::new(
        args.first().ok_or("verification_command_missing")?,
    ));
    command
        .args(&args[1..])
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| "verification_launch_failed")?;
    let done = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    let output = capture(child.stdout.take().unwrap(), done.clone());
    #[cfg(unix)]
    let errors = capture(child.stderr.take().unwrap(), done.clone());
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut interrupted = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|_| "verification_wait_failed")? {
            break status;
        }
        if Instant::now() > deadline || cancel() {
            interrupted = true;
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
            break child.wait().map_err(|_| "verification_wait_failed")?;
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    done.store(true, Ordering::Release);
    #[cfg(unix)]
    let (out, err) = (
        output.join().map_err(|_| "verification_output_failed")?,
        errors.join().map_err(|_| "verification_output_failed")?,
    );
    #[cfg(not(unix))]
    let (out, err) = (
        json!({"streamIncomplete":true}),
        json!({"streamIncomplete":true}),
    );
    Ok(
        json!({"argv":args,"exitCode":status.code(),"timeout":interrupted,"output":out["text"],"error":err["text"],"outputSha256":out["sha256"],"outputBytes":out["bytes"],"truncated":out["truncated"]==true||err["truncated"]==true,"streamIncomplete":out["streamIncomplete"]==true||err["streamIncomplete"]==true}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hash_covers_output_beyond_the_display_limit() {
        let root = tempfile::tempdir().unwrap();
        let args = vec![
            "/usr/bin/python3".into(),
            "-c".into(),
            "print('x'*20000,end='')".into(),
        ];
        let result = check(root.path(), &args, 5, &|| false).unwrap();
        assert_eq!(result["outputBytes"], 20000);
        assert_eq!(result["truncated"], true);
        assert_eq!(
            result["outputSha256"],
            format!("{:x}", Sha256::digest("x".repeat(20000).as_bytes()))
        );
    }
    #[test]
    fn cancellation_terminates_owned_command_and_records_incomplete() {
        let root = tempfile::tempdir().unwrap();
        let result = check(root.path(), &["/bin/sleep".into(), "30".into()], 5, &|| {
            true
        })
        .unwrap();
        assert_eq!(result["timeout"], true);
        assert_ne!(result["exitCode"], 0);
    }
}
