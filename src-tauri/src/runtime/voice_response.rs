//! Lightweight voice surface. Canonical reasoning, tools, and persistence stay on the main turn.
use super::event_hub::{RuntimeEventSender, TurnEventHub};
use crate::{
    ipc_contract::{ConversationMessage, RuntimeEvent},
    AppState, RunCancellation, StartTurnInput,
};
use std::{sync::Arc, time::Instant};

#[path = "voice_response_state.rs"]
mod state;
pub(crate) use state::HubState;

fn language(state: &AppState) -> String {
    state
        .sqlite_readers
        .read(|connection| {
            Ok(crate::persistence::settings::regional_preferences::load(connection)?.language)
        })
        .ok()
        .filter(|language| matches!(language.as_str(), "ja" | "en"))
        .unwrap_or_else(|| "auto".into())
}

fn speech_allowed(state: &AppState, input: &StartTurnInput) -> bool {
    crate::voice_behavior::completion_state(state, &input.run_id, &input.conversation_id)
        .0
        .decision
        == "speak"
}

pub(crate) async fn complete(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    message: &ConversationMessage,
) -> Result<(), String> {
    if !on_event.voice_response_enabled()
        || input.input_origin != "voice"
        || cancellation.is_cancelled()
        || !speech_allowed(state, input)
    {
        return crate::memory::personal_state::output::send_completed(
            state, input, on_event, message,
        );
    }
    if let Ok(speech) = crate::larm_voice::render_response(
        &input.conversation_id,
        crate::larm_voice::ResponseKind::Final,
        &message.content,
        &language(state),
        cancellation,
    )
    .await
    {
        on_event.set_completion_speech(&input.run_id, speech);
    }
    crate::memory::personal_state::output::send_completed(state, input, on_event, message)
}

impl RuntimeEventSender for TurnEventHub {
    fn wait_message_presented<'a>(
        &'a self,
        state: &'a AppState,
        message_id: &'a str,
        cancellation: Arc<RunCancellation>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(self.wait_presented(state, message_id, cancellation))
    }
    fn voice_response_enabled(&self) -> bool {
        self.voice_response.enabled()
    }

    fn speak_voice_response(
        &self,
        run_id: &str,
        kind: crate::larm_voice::ResponseKind,
        text: &str,
    ) -> Result<(), String> {
        if !self.voice_response.enabled() || !self.streaming_speech {
            return Ok(());
        }
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        self.voice_response
            .queue_in_sequence(run_id, kind, || self.speech.queue_utterance(run_id, text))
    }

    fn set_completion_speech(&self, run_id: &str, text: String) {
        self.voice_response.set_completion(run_id, text);
    }

    fn acknowledge<'a>(
        &'a self,
        state: &'a AppState,
        run_id: &'a str,
        conversation_id: &'a str,
        cancellation: Arc<RunCancellation>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(super::event_hub::reasoning_ack::speak(
            self,
            state,
            run_id,
            conversation_id,
            cancellation,
        ))
    }

    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        self.dispatch(event, None)
    }

    fn send_received(&self, event: RuntimeEvent, received_at: Instant) -> tauri::Result<()> {
        self.dispatch(event, Some(received_at))
    }

    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
}
