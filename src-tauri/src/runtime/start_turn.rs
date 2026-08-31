use std::sync::Arc;

use crate::ipc_contract::RuntimeEvent;
use crate::{
    execute_turn, persistence, redact_runtime_text, register_active_run, validate_identifier,
    validate_start_turn, voice_behavior, AppState, RunCancellation, StartTurnInput,
};

#[tauri::command]
pub(crate) async fn start_turn(
    state: tauri::State<'_, AppState>,
    input: StartTurnInput,
    on_event: tauri::ipc::Channel<RuntimeEvent>,
) -> Result<(), String> {
    validate_start_turn(&input)?;
    validate_identifier(
        input.source_id.as_deref().unwrap_or("none"),
        "turn source id",
    )?;
    let _ = persistence::audit::record_turn_request(&state, &input);
    let (mut streaming_speech, speech_enabled) =
        voice_behavior::begin_turn_speech_policy(&state, &input)?;
    let cancellation = Arc::new(RunCancellation::default());
    if let Err(error) = register_active_run(&state, &input.run_id, cancellation.clone()) {
        voice_behavior::end_run(&state, &input.run_id);
        return Err(error);
    }
    if streaming_speech {
        if let Err(error) = state
            .streaming_tts
            .begin(&state, &input.run_id, speech_enabled, on_event.clone())
            .await
        {
            streaming_speech = false;
            let _ = on_event.send(RuntimeEvent::SpeechFailed {
                run_id: input.run_id.clone(),
                message: redact_runtime_text(&error),
                recovery: "Check the speech provider and try another response.".to_string(),
            });
        }
    }
    let event_hub = crate::runtime::event_hub::TurnEventHub::new(
        on_event,
        state.streaming_tts.clone(),
        streaming_speech,
    );
    let result = execute_turn(&state, &input, &event_hub, cancellation.clone(), None).await;
    if result.is_err() && streaming_speech {
        state.streaming_tts.cancel(&input.run_id);
    }
    if let Ok(mut active) = state.active_runs.lock() {
        active.remove(&input.run_id);
    }
    voice_behavior::end_run(&state, &input.run_id);
    result.map_err(|error| redact_runtime_text(&error.message))
}
