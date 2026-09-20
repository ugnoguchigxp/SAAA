use super::*;

pub(super) struct AppState {
    pub(super) sqlite_writer: Arc<SqliteWriter>,
    pub(super) sqlite_readers: SqliteReaders,
    pub(super) data_directory: PathBuf,
    pub(super) context_still_recall: memory::context_still_recall::ContextStillRecallClient,
    pub(super) active_runs: Mutex<HashMap<String, Arc<RunCancellation>>>,
    pub(super) provider_probes: Mutex<HashMap<String, ProviderProbeStatus>>,
    pub(super) interaction_policy: Mutex<()>,
    pub(super) shutdown_started: AtomicBool,
    pub(super) audio_uploads: voice::audio_upload::AudioUploadStore,
    pub(super) streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime,
    pub(super) voice_behavior: voice_behavior::VoiceBehaviorRuntime,
    pub(super) situation: Arc<situation::SituationRuntime>,
    pub(super) voice_profile: Arc<voice::profile::VoiceProfileRuntime>,
    pub(super) voice_asr: AsrSessionManager,
    pub(super) generated_capabilities: Arc<generated_capabilities::service::CapabilityService>,
    pub(super) generation:
        Option<Arc<generated_capabilities::generation::service::GenerationService>>,
    pub(super) generated_tools: generated_capabilities::publication::GeneratedToolsConfig,
    pub(super) tool_selection: Arc<tool_selection::ToolSelectionService>,
    pub(super) mcp_server: Mutex<Option<tool_selection::mcp_server::ServerHandle>>,
    pub(super) schedule: std::sync::Arc<crate::schedule::Handle>,
}

#[derive(Clone)]
pub(super) struct ProviderProbeStatus {
    pub(super) ok: bool,
    pub(super) checked_at: String,
    pub(super) configuration_fingerprint: String,
    pub(super) prior_session_rowid: i64,
}

/// Cheap-to-clone cancellation handle. The state lives behind an `Arc` so a management task can
/// own a handle and observe cancellation even after the original caller future is dropped.
#[derive(Clone, Default)]
pub(super) struct RunCancellation {
    inner: Arc<RunCancellationInner>,
}

#[derive(Default)]
struct RunCancellationInner {
    acceptance: Mutex<()>,
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl RunCancellation {
    pub(crate) fn cancel(&self) {
        let _acceptance = self
            .inner
            .acceptance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
            self.inner.notify.notify_one();
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) fn with_active<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _acceptance = self
            .inner
            .acceptance
            .lock()
            .map_err(|_| "Run acceptance lock unavailable")?;
        if self.is_cancelled() {
            return Err("Cancelled by user".into());
        }
        action()
    }

    pub(super) async fn cancelled(&self) {
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}
