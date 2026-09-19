use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{mpsc, Notify},
    task::JoinHandle,
    time::{sleep_until, Instant},
};

use crate::generated_capabilities::limits::{MAX_STDERR_BYTES, MAX_STDOUT_BYTES};

#[derive(Clone, Debug, Default)]
pub struct Cancellation {
    inner: Arc<CancellationInner>,
}

#[derive(Debug, Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancellation {
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
            self.inner.notify.notify_one();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.inner.cancelled.load(Ordering::SeqCst) {
            return;
        }
        notified.await;
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeCommand {
    pub executable: PathBuf,
    pub arguments: Vec<PathBuf>,
    pub current_dir: PathBuf,
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportErrorKind {
    Start,
    Send,
    StdoutLimit,
    StderrLimit,
    Timeout,
    Cancelled,
    Exit,
    Wait,
    Protocol,
}

#[derive(Debug)]
pub struct TransportError {
    pub kind: TransportErrorKind,
    pub message: String,
    pub exit_code: Option<i32>,
    pub stderr: String,
    pub child_pid: Option<u32>,
    pub reaped: bool,
}

impl TransportError {
    fn new(kind: TransportErrorKind, message: impl Into<String>, pid: Option<u32>) -> Self {
        Self {
            kind,
            message: message.into(),
            exit_code: None,
            stderr: String::new(),
            child_pid: pid,
            reaped: false,
        }
    }
}

enum PipeEvent {
    Stdout(Result<Vec<u8>, std::io::Error>),
    Stderr(Result<Vec<u8>, std::io::Error>),
}

pub async fn execute(
    command: &RuntimeCommand,
    request: &[u8],
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<ProcessOutput, TransportError> {
    if !command.executable.is_absolute()
        || command.arguments.iter().any(|path| !path.is_absolute())
        || !command.current_dir.is_absolute()
    {
        return Err(TransportError::new(
            TransportErrorKind::Start,
            "runtime command paths must be absolute",
            None,
        ));
    }
    let mut process = Command::new(&command.executable);
    process
        .args(&command.arguments)
        .current_dir(&command.current_dir)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = process.spawn().map_err(|error| {
        TransportError::new(
            TransportErrorKind::Start,
            format!("runtime could not be started: {error}"),
            None,
        )
    })?;
    let pid = child.id();
    let Some(mut stdin) = child.stdin.take() else {
        return abort(
            &mut child,
            TransportError::new(
                TransportErrorKind::Start,
                "runtime stdin is unavailable",
                pid,
            ),
            None,
        )
        .await;
    };
    let Some(stdout) = child.stdout.take() else {
        return abort(
            &mut child,
            TransportError::new(
                TransportErrorKind::Start,
                "runtime stdout is unavailable",
                pid,
            ),
            None,
        )
        .await;
    };
    let Some(stderr) = child.stderr.take() else {
        return abort(
            &mut child,
            TransportError::new(
                TransportErrorKind::Start,
                "runtime stderr is unavailable",
                pid,
            ),
            None,
        )
        .await;
    };
    let deadline = Instant::now() + timeout;
    let send = async {
        stdin.write_all(request).await?;
        stdin.shutdown().await
    };
    let mut send = Box::pin(send);
    let send_result = tokio::select! { biased;
        _ = cancellation.cancelled() => {
            return abort(&mut child, TransportError::new(TransportErrorKind::Cancelled, "runtime cancelled while sending request", pid), None).await;
        }
        _ = sleep_until(deadline) => {
            return abort(&mut child, TransportError::new(TransportErrorKind::Timeout, "runtime timed out while sending request", pid), None).await;
        }
        result = send.as_mut() => result,
    };
    drop(send);
    drop(stdin);
    if let Err(error) = send_result {
        return abort(
            &mut child,
            TransportError::new(
                TransportErrorKind::Send,
                format!("runtime request could not be sent: {error}"),
                pid,
            ),
            None,
        )
        .await;
    }

    let (sender, mut receiver) = mpsc::unbounded_channel();
    let stdout_task = spawn_reader(stdout, MAX_STDOUT_BYTES, true, sender.clone());
    let stderr_task = spawn_reader(stderr, MAX_STDERR_BYTES, false, sender);
    let mut stdout_value = None;
    let mut stderr_value = None;
    let mut status = None;

    while stdout_value.is_none() || stderr_value.is_none() || status.is_none() {
        tokio::select! { biased;
            _ = cancellation.cancelled() => {
                let error = TransportError::new(TransportErrorKind::Cancelled, "runtime was cancelled", pid);
                return abort_with_readers(&mut child, error, &stdout_task, &stderr_task).await;
            }
            _ = sleep_until(deadline) => {
                let error = TransportError::new(TransportErrorKind::Timeout, "runtime exceeded its host deadline", pid);
                return abort_with_readers(&mut child, error, &stdout_task, &stderr_task).await;
            }
            result = child.wait(), if status.is_none() => {
                match result {
                    Ok(value) => status = Some(value),
                    Err(error) => {
                        let failure = TransportError::new(TransportErrorKind::Wait, format!("runtime wait failed: {error}"), pid);
                        return abort_with_readers(&mut child, failure, &stdout_task, &stderr_task).await;
                    }
                }
            }
            event = receiver.recv(), if stdout_value.is_none() || stderr_value.is_none() => {
                match event {
                    Some(PipeEvent::Stdout(Ok(value))) => stdout_value = Some(value),
                    Some(PipeEvent::Stderr(Ok(value))) => stderr_value = Some(value),
                    Some(PipeEvent::Stdout(Err(error))) => {
                        let failure = TransportError::new(TransportErrorKind::StdoutLimit, format!("runtime stdout exceeded its limit: {error}"), pid);
                        return abort_with_readers(&mut child, failure, &stdout_task, &stderr_task).await;
                    }
                    Some(PipeEvent::Stderr(Err(error))) => {
                        let failure = TransportError::new(TransportErrorKind::StderrLimit, format!("runtime stderr exceeded its limit: {error}"), pid);
                        return abort_with_readers(&mut child, failure, &stdout_task, &stderr_task).await;
                    }
                    None => {
                        let failure = TransportError::new(TransportErrorKind::Wait, "runtime output readers stopped unexpectedly", pid);
                        return abort_with_readers(&mut child, failure, &stdout_task, &stderr_task).await;
                    }
                }
            }
        }
    }
    join_reader(stdout_task).await;
    join_reader(stderr_task).await;
    let stderr = stderr_value.unwrap_or_default();
    let status = status.expect("loop requires child status");
    if !status.success() {
        let mut error = TransportError::new(
            TransportErrorKind::Exit,
            "runtime exited unsuccessfully",
            pid,
        );
        error.exit_code = status.code();
        error.stderr = String::from_utf8_lossy(&stderr).into_owned();
        error.reaped = true;
        return Err(error);
    }
    Ok(ProcessOutput {
        stdout: stdout_value.unwrap_or_default(),
        stderr,
    })
}

fn spawn_reader<R: AsyncRead + Unpin + Send + 'static>(
    reader: R,
    limit: usize,
    stdout: bool,
    sender: mpsc::UnboundedSender<PipeEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let result = read_bounded(reader, limit).await;
        let event = if stdout {
            PipeEvent::Stdout(result)
        } else {
            PipeEvent::Stderr(result)
        };
        let _ = sender.send(event);
    })
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut result = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(result);
        }
        if result.len().saturating_add(count) > limit {
            return Err(std::io::Error::other("bounded output limit exceeded"));
        }
        result.extend_from_slice(&chunk[..count]);
    }
}

async fn abort_with_readers<T>(
    child: &mut Child,
    error: TransportError,
    stdout: &JoinHandle<()>,
    stderr: &JoinHandle<()>,
) -> Result<T, TransportError> {
    let result = abort(child, error, None).await;
    wait_or_abort_reader(stdout).await;
    wait_or_abort_reader(stderr).await;
    result
}

async fn abort<T>(
    child: &mut Child,
    mut error: TransportError,
    stderr: Option<&[u8]>,
) -> Result<T, TransportError> {
    let _ = child.start_kill();
    error.reaped = child.wait().await.is_ok();
    if let Some(stderr) = stderr {
        error.stderr = String::from_utf8_lossy(stderr).into_owned();
    }
    Err(error)
}

async fn wait_or_abort_reader(task: &JoinHandle<()>) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !task.is_finished() && Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    if !task.is_finished() {
        task.abort();
    }
}

async fn join_reader(task: JoinHandle<()>) {
    let _ = task.await;
}
