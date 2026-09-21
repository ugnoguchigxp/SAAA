use crate::coding::contracts::CodingSettings;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
fn launch_command(settings: &CodingSettings) -> Command {
    let mut command = Command::new(&settings.executable);
    // Node and Codex installed beside pi must also resolve when launched by Finder.
    if let Some(directory) = Path::new(&settings.executable).parent() {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let paths =
            std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&inherited));
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    command
}
pub const LINE_LIMIT: usize = 8 * 1024 * 1024;
static SLOT: Mutex<()> = Mutex::new(());
const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

pub fn record(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    Ok(record_counted(reader)?.map(|(value, _)| value))
}
fn record_counted(reader: &mut impl BufRead) -> Result<Option<(Value, usize)>, String> {
    let mut line = Vec::new();
    let count = reader
        .take((LINE_LIMIT + 1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|_| "rpc_read_failed")?;
    if count == 0 {
        return Ok(None);
    }
    if count > LINE_LIMIT {
        return Err("rpc_line_limit".into());
    }
    if line.last() != Some(&b'\n') {
        return Err("rpc_truncated_record".into());
    }
    let value: Value = serde_json::from_slice(&line).map_err(|_| "rpc_invalid_json")?;
    if !value.is_object() {
        return Err("rpc_invalid_record".into());
    }
    Ok(Some((value, count)))
}
pub struct Process {
    _slot: MutexGuard<'static, ()>,
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Result<Value, String>>,
    readers: Vec<std::thread::JoinHandle<()>>,
    sequence: u64,
    exited: bool,
}
impl Process {
    pub fn open(settings: &CodingSettings, cwd: &Path, session: &Path) -> Result<Self, String> {
        let slot = SLOT.try_lock().map_err(|_| "busy")?;
        let mut command = if settings.profile == "delegated-read-test-macos-v1" {
            delegated_command(settings, cwd, session)?
        } else {
            launch_command(settings)
        };
        command
            .args(["--mode", "rpc", "--session"])
            .arg(session)
            .args([
                "--provider",
                &settings.provider,
                "--model",
                &settings.model,
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-context-files",
                "--no-approve",
            ])
            .current_dir(cwd)
            .env("PI_SKIP_VERSION_CHECK", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        if settings.profile == "codex-sdk-v1" {
            if !crate::coding::contracts::valid_profile(settings) {
                return Err("coding_configuration_invalid".into());
            }
            command.arg("--extension").arg(
                settings
                    .sdk_extension_path
                    .as_ref()
                    .ok_or("sdk_extension_missing")?,
            );
        }
        let mut child = command.spawn().map_err(|_| "pi_start_failed")?;
        let stdin = child.stdin.take();
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = stdin.as_ref().ok_or("pi_stdin_missing")?.as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err("pi_pipe_configuration_failed".into());
            }
        }
        let stdout = child.stdout.take().ok_or("pi_stdout_missing")?;
        let stderr = child.stderr.take().ok_or("pi_stderr_missing")?;
        let (tx, rx) = mpsc::sync_channel(64);
        let bytes = Arc::new(AtomicUsize::new(0));
        let counted = bytes.clone();
        let errors = tx.clone();
        let out = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match record_counted(&mut reader) {
                    Ok(Some((value, count))) => {
                        if counted.fetch_add(count, Ordering::Relaxed) + count > OUTPUT_LIMIT {
                            let _ = tx.send(Err("rpc_output_limit".into()));
                            break;
                        }
                        if tx.send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        let err = std::thread::spawn(move || {
            let mut reader = stderr;
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if bytes.fetch_add(n, Ordering::Relaxed) + n > OUTPUT_LIMIT {
                            let _ = errors.send(Err("rpc_output_limit".into()));
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            _slot: slot,
            child,
            stdin,
            rx,
            readers: vec![out, err],
            sequence: 0,
            exited: false,
        })
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn send(&mut self, kind: &str, fields: Value) -> Result<String, String> {
        self.sequence += 1;
        let id = format!("saaa-{}", self.sequence);
        let mut value = fields;
        value["id"] = json!(id);
        value["type"] = json!(kind);
        let stdin = self.stdin.as_mut().ok_or("rpc_closed")?;
        let wire = format!("{value}\n");
        let mut remaining = wire.as_bytes();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !remaining.is_empty() {
            if Instant::now() > deadline {
                return Err("rpc_write_timeout".into());
            }
            match stdin.write(remaining) {
                Ok(0) => return Err("rpc_write_failed".into()),
                Ok(count) => remaining = &remaining[count..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err("rpc_write_failed".into()),
            }
        }
        Ok(id)
    }
    pub fn next(&self, timeout: Duration) -> Result<Option<Value>, String> {
        match self.rx.recv_timeout(timeout) {
            Ok(v) => v.map(Some),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(_) => Err("rpc_process_exited".into()),
        }
    }
    pub fn command(
        &mut self,
        kind: &str,
        fields: Value,
        cancelled: impl Fn() -> Result<bool, String>,
    ) -> Result<Value, String> {
        let id = self.send(kind, fields)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if cancelled()? {
                return Err("cancelled_before_acceptance".into());
            }
            if Instant::now() >= deadline {
                return Err("rpc_acceptance_timeout".into());
            }
            if let Some(value) = self.next(Duration::from_millis(100))? {
                if value["type"] == "response" && value["id"] == id {
                    if value["success"] != true {
                        return Err("rpc_rejected".into());
                    }
                    return Ok(value["data"].clone());
                }
            }
        }
    }
    pub fn close(&mut self) -> Result<(), String> {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut error = None;
        loop {
            while let Ok(v) = self.rx.try_recv() {
                if let Err(e) = v {
                    error = Some(e);
                }
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.exited = true;
                    if !status.success() {
                        error = Some("pi_nonzero_exit".into());
                    }
                    break;
                }
                Err(_) => {
                    error = Some("pi_wait_failed".into());
                    break;
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                error = Some("pi_cleanup_timeout".into());
                self.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Reap any remaining tools in the group created for this child.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        // Keep draining while pipe readers finish, including grandchildren holding descriptors.
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.readers.iter().any(|r| !r.is_finished()) && Instant::now() < deadline {
            while self.rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.readers.iter().any(|r| !r.is_finished()) {
            error = Some("pi_pipe_cleanup_incomplete".into());
        }
        for reader in self.readers.drain(..) {
            if reader.is_finished() {
                let _ = reader.join();
            }
        }
        error.map_or(Ok(()), Err)
    }
    fn kill(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.exited = true;
    }
}

pub(crate) fn delegated_command(
    settings: &CodingSettings,
    _workspace: &Path,
    session: &Path,
) -> Result<Command, String> {
    #[cfg(target_os = "macos")]
    {
        let session_dir = session.parent().ok_or("delegated_profile_invalid")?;
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("delegated_profile_invalid")?;
        let agent_dir = home.join(".pi").join("agent");
        let quote = |path: &Path| format!("\"{}\"", path.to_string_lossy().replace('"', "\\\""));
        // sandbox-exec applies to pi and every descendant process. Read access
        // is broad enough for a compiler/test runner, while writes are limited
        // to the session and temporary directories and network is absent.
        // Node 24 queries kernel facts during allocator initialization.  That
        // operation is read-only, but is separately mediated by Seatbelt. Pi
        // also creates lock directories while it reads its existing local
        // settings and credentials. Those named lock directories are the only
        // home-directory writes granted; they cannot modify either JSON file.
        let profile = format!(
            "(version 1) (deny default) (allow process*) (allow sysctl-read) (allow file-read*) (allow file-write* (subpath {}) (subpath {}) (subpath {}) (subpath \"/private/tmp\") (subpath \"/tmp\") (subpath \"/dev\"))",
            quote(session_dir),
            quote(&agent_dir.join("settings.json.lock")),
            quote(&agent_dir.join("auth.json.lock")),
        );
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command.args(["-p", &profile]).arg(&settings.executable);
        Ok(command)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (settings, workspace, session);
        Err("delegated_profile_unsupported".into())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if !self.exited {
            self.kill();
        }
    }
}

pub fn check_settings(settings: &CodingSettings) -> Result<(), String> {
    let _slot = SLOT.try_lock().map_err(|_| "busy")?;
    if settings.version != "0.86.1"
        || !crate::coding::contracts::valid_profile(settings)
        || !Path::new(&settings.executable).is_absolute()
        || settings.provider.is_empty()
        || settings.model.is_empty()
    {
        return Err("coding_configuration_invalid".into());
    }
    let mut child = launch_command(settings)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "pi_unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().map_err(|_| "pi_probe_failed")?.is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("pi_version_timeout".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut output = String::new();
    child
        .stdout
        .take()
        .ok_or("pi_probe_failed")?
        .take(1024)
        .read_to_string(&mut output)
        .map_err(|_| "pi_probe_failed")?;
    if output.trim() != settings.version {
        return Err("pi_version_mismatch".into());
    }
    Ok(())
}
pub fn ready(
    process: &mut Process,
    settings: &CodingSettings,
    session: &Path,
) -> Result<Value, String> {
    ready_cancellable(process, settings, session, || Ok(false))
}
pub fn ready_cancellable(
    process: &mut Process,
    settings: &CodingSettings,
    session: &Path,
    cancelled: impl Fn() -> Result<bool, String>,
) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let check = || {
        if Instant::now() > deadline {
            Err("pi_startup_timeout".into())
        } else {
            cancelled()
        }
    };
    let state = process.command("get_state", json!({}), check)?;
    let models = process.command("get_available_models", json!({}), check)?;
    let available = models["models"].as_array().ok_or("rpc_invalid_models")?;
    if !available
        .iter()
        .any(|m| m["provider"] == settings.provider && m["id"] == settings.model)
    {
        return Err(if available.is_empty() {
            "authentication_required"
        } else {
            "model_unavailable"
        }
        .into());
    }
    if state["sessionFile"].as_str() != session.to_str()
        || state["sessionId"]
            .as_str()
            .is_none_or(|id| id.is_empty() || id.len() > 160)
        || state["model"]["provider"] != settings.provider
        || state["model"]["id"] != settings.model
        || state["isStreaming"] != false
    {
        return Err("pi_state_mismatch".into());
    }
    if settings.profile == "codex-sdk-v1" {
        process.command("set_auto_compaction", json!({"enabled":false}), check)?;
        process.command("set_auto_retry", json!({"enabled":false}), check)?;
    }
    Ok(state)
}

pub fn probe(settings: &CodingSettings, directory: &Path) -> Result<(), String> {
    check_settings(settings)?;
    let temporary = tempfile::tempdir_in(directory).map_err(|_| "probe_storage_unavailable")?;
    let session = temporary.path().join("probe.jsonl");
    let mut child = Process::open(settings, temporary.path(), &session)?;
    let result = ready(&mut child, settings, &session);
    let closed = child.close();
    result?;
    closed
}
