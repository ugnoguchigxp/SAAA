//! Explicit boundary while the removed speech scheduler is replaced.
use crate::{ipc_contract::RuntimeEvent, AppState};

#[derive(Default)]
pub(crate) struct UnavailableSpeechRuntime;

impl UnavailableSpeechRuntime {
    pub(crate) fn is_active(&self) -> bool {
        false
    }

    pub(crate) fn set_enabled(&self, _run_id: &str, _enabled: bool) {}

    pub(crate) fn cancel(&self, _run_id: &str) {}

    pub(crate) fn shutdown(&self) {}

    pub(crate) async fn begin(
        &self,
        _state: &AppState,
        _run_id: &str,
        _enabled: bool,
        _channel: tauri::ipc::Channel<RuntimeEvent>,
        _conversation_id: Option<&str>,
    ) -> Result<(), String> {
        Err("回答音声ランタイムは再構築中です。".into())
    }

    pub(crate) fn queue_utterance(&self, _run_id: &str, _text: &str) -> Result<(), String> {
        Err("回答音声ランタイムは再構築中です。".into())
    }

    pub(crate) fn finish(&self, _run_id: &str, _digest: &str) -> Result<(), String> {
        Err("回答音声ランタイムは再構築中です。".into())
    }
}
