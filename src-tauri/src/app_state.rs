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
    pub(super) network_asr: voice::network_asr::NetworkAsrRuntime,
    pub(super) audio_uploads: voice::audio_upload::AudioUploadStore,
    pub(super) streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime,
    pub(super) voice_behavior: voice_behavior::VoiceBehaviorRuntime,
    pub(super) situation: Arc<situation::SituationRuntime>,
    pub(super) meeting: Arc<meeting::MeetingRuntime>,
    pub(super) voice_profile: Arc<voice::profile::VoiceProfileRuntime>,
    pub(super) voice_asr: AsrSessionManager,
    pub(super) generated_capabilities: Arc<generated_capabilities::service::CapabilityService>,
    pub(super) generated_tools: generated_capabilities::publication::GeneratedToolsConfig,
    pub(super) tool_selection: Arc<tool_selection::ToolSelectionService>,
}

#[derive(Clone)]
pub(super) struct ProviderProbeStatus {
    pub(super) ok: bool,
    pub(super) checked_at: String,
    pub(super) configuration_fingerprint: String,
    pub(super) prior_session_rowid: i64,
}

#[derive(Default)]
pub(super) struct RunCancellation {
    pub(super) acceptance: Mutex<()>,
    pub(super) cancelled: AtomicBool,
    pub(super) notify: tokio::sync::Notify,
}

impl RunCancellation {
    pub(crate) fn cancel(&self) {
        let _acceptance = self
            .acceptance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            self.notify.notify_waiters();
            self.notify.notify_one();
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) fn with_active<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _acceptance = self
            .acceptance
            .lock()
            .map_err(|_| "Run acceptance lock unavailable")?;
        if self.is_cancelled() {
            return Err("Cancelled by user".into());
        }
        action()
    }

    pub(super) async fn cancelled(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}
