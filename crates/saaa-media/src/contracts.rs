//! Host-facing results. These types are not the persisted JSON or the Tauri IPC payload.
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

use saaa_larm_session::media::MediaProgress;
use tokio::sync::{mpsc, watch};
use zeroize::Zeroizing;

use crate::GenerateOutput;

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

#[derive(Clone, PartialEq, Eq)]
pub struct MediaHostError {
    pub code: &'static str,
    pub message: String,
}

impl MediaHostError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }

    pub fn duplicate(message: impl Into<String>) -> Self {
        Self::new("duplicate_run", message)
    }

    pub fn capacity() -> Self {
        Self::new("capacity", "生成要求を処理中です。完了までお待ちください。")
    }

    pub fn route(message: impl Into<String>) -> Self {
        Self::new("route_unavailable", message)
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::new("storage_failed", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new("in_progress", message)
    }

    pub fn http_status(&self) -> u16 {
        match self.code {
            "invalid_input" => 400,
            "unauthenticated" => 401,
            "forbidden" => 403,
            "not_found" => 404,
            "duplicate_run" | "in_progress" => 409,
            "payload_too_large" => 413,
            "unsupported_media" => 415,
            "capacity" => 429,
            "route_unavailable" => 422,
            _ => 500,
        }
    }
}

impl fmt::Debug for MediaHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaHostError")
            .field("code", &self.code)
            .field("message", &self.message)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum RunTerminal {
    Output(GenerateOutput),
    HostFailure(MediaHostError),
}

impl RunTerminal {
    pub fn into_command(self) -> Result<GenerateOutput, String> {
        match self {
            Self::Output(output) => Ok(output),
            Self::HostFailure(error) => Err(error.message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryQuery {
    Latest { limit: u32 },
    ByRunId(String),
}

impl Default for HistoryQuery {
    fn default() -> Self {
        Self::Latest { limit: 20 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactBytes {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

pub fn allowed_mime(value: &str) -> bool {
    matches!(
        value,
        "image/png"
            | "image/webp"
            | "image/jpeg"
            | "audio/ogg"
            | "audio/mpeg"
            | "audio/wav"
            | "audio/flac"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelStatus {
    Accepted,
    AlreadyTerminal,
}

/// Remote stop is separate from whether the cancel request was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteStop {
    NotAttempted,
    Confirmed,
    Unconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelOutcome {
    pub status: CancelStatus,
    pub remote_stop: RemoteStop,
}

/// Dropping the handle unsubscribes. It does not cancel the run.
pub struct RunHandle {
    run_id: String,
    progress: mpsc::Receiver<MediaProgress>,
    terminal: watch::Receiver<Option<RunTerminal>>,
}

impl RunHandle {
    pub(crate) fn new(
        run_id: String,
        progress: mpsc::Receiver<MediaProgress>,
        terminal: watch::Receiver<Option<RunTerminal>>,
    ) -> Self {
        Self {
            run_id,
            progress,
            terminal,
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn terminal(&self) -> Option<RunTerminal> {
        self.terminal.borrow().clone()
    }

    pub async fn next_progress(&mut self) -> Option<MediaProgress> {
        self.progress.recv().await
    }

    pub async fn wait_terminal(&mut self) -> RunTerminal {
        loop {
            if let Some(terminal) = self.terminal() {
                return terminal;
            }
            if self.terminal.changed().await.is_err() {
                return RunTerminal::HostFailure(MediaHostError::storage(
                    "生成結果の待受が切れました。",
                ));
            }
        }
    }
}

pub fn validate_larm_token(value: &str) -> Result<(), MediaHostError> {
    if value.is_empty()
        || value.len() > 4_096
        || value.trim().is_empty()
        || value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    {
        Err(MediaHostError::new(
            "credential_invalid",
            "LARMの資格情報を確認してください。",
        ))
    } else {
        Ok(())
    }
}

pub fn validate_named_secret(
    service: &str,
    account: &str,
    value: &str,
) -> Result<(), MediaHostError> {
    if service.is_empty() || account.is_empty() || value.is_empty() || value.len() > 2_560 {
        Err(MediaHostError::new(
            "credential_invalid",
            "資格情報を確認してください。",
        ))
    } else {
        Ok(())
    }
}

/// Explicit lab secrets. Debug output never includes the values.
#[derive(Clone)]
pub struct ExplicitSecrets {
    token: Option<Zeroizing<String>>,
    named: Vec<(String, String, Zeroizing<String>)>,
}

impl ExplicitSecrets {
    pub fn new(
        token: Option<String>,
        named: Vec<(String, String, String)>,
    ) -> Result<Self, MediaHostError> {
        if let Some(token) = &token {
            validate_larm_token(token)?;
        }
        for (service, account, value) in &named {
            validate_named_secret(service, account, value)?;
        }
        Ok(Self {
            token: token.map(Zeroizing::new),
            named: named
                .into_iter()
                .map(|(service, account, value)| (service, account, Zeroizing::new(value)))
                .collect(),
        })
    }

    pub fn larm_token(&self) -> Result<Zeroizing<String>, MediaHostError> {
        self.token
            .as_ref()
            .map(|token| Zeroizing::new(token.to_string()))
            .ok_or_else(|| MediaHostError::route("LARMの資格情報が設定されていません。"))
    }

    pub fn named(&self, service: &str, account: &str) -> Option<Zeroizing<String>> {
        self.named
            .iter()
            .find(|(stored_service, stored_account, _)| {
                stored_service == service && stored_account == account
            })
            .map(|(_, _, value)| Zeroizing::new(value.to_string()))
    }
}

impl fmt::Debug for ExplicitSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExplicitSecrets")
            .field("larmToken", &self.token.as_ref().map(|_| "<redacted>"))
            .field("named", &self.named.len())
            .finish()
    }
}

pub trait Clock: Send + Sync {
    fn now_iso(&self) -> String;
    fn sleep(&self, duration: Duration) -> BoxFut<()>;
}

pub struct ManualClock {
    now: Mutex<String>,
    expire: AtomicBool,
    released: std::sync::Arc<tokio::sync::Notify>,
}

impl ManualClock {
    pub fn new(now: impl Into<String>) -> Self {
        Self {
            now: Mutex::new(now.into()),
            expire: AtomicBool::new(false),
            released: std::sync::Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn expire_immediately(&self) {
        self.expire.store(true, Ordering::SeqCst);
        self.released.notify_waiters();
    }

    pub fn release_sleepers(&self) {
        self.released.notify_waiters();
    }
}

impl Clock for ManualClock {
    fn now_iso(&self) -> String {
        self.now
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    fn sleep(&self, _duration: Duration) -> BoxFut<()> {
        if self.expire.load(Ordering::SeqCst) {
            Box::pin(async {})
        } else {
            let released = self.released.clone();
            Box::pin(async move {
                released.notified().await;
            })
        }
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_iso(&self) -> String {
        let milliseconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("{milliseconds}")
    }

    fn sleep(&self, duration: Duration) -> BoxFut<()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
