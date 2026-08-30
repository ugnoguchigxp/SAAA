use super::contracts::{
    CommitVoiceAsrUtteranceInput, StartVoiceAsrSessionInput, StopVoiceAsrSessionInput,
    VoiceAsrStreamEvent,
};
use crate::AppState;
use tauri::{
    ipc::{InvokeBody, Request},
    State,
};
const SESSION: &str = "x-saaa-asr-session-id";
const SEQUENCE: &str = "x-saaa-asr-sequence";
const SAMPLE_COUNT: &str = "x-saaa-asr-sample-count";
#[tauri::command]
pub(crate) async fn start_voice_asr_session(
    state: State<'_, AppState>,
    input: StartVoiceAsrSessionInput,
    on_event: tauri::ipc::Channel<VoiceAsrStreamEvent>,
) -> Result<(), String> {
    crate::voice::session::probe_selected_asr(&state)
        .await
        .map_err(|_| "asr-provider-unavailable".to_string())?;
    let verifier = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        state
            .voice_profile
            .prepare_streaming_verifier(&connection)?
    };
    // The capture manager currently sends only the agreement-safe batch
    // protocol. Do not advertise a native stream until its transport actor is
    // connected to append/commit and can produce provider events.
    let protocol = "batch-agreement";
    // Passing unfiltered microphone audio to an ASR provider would violate the
    // target-speaker setting. Keep this mode fail-closed until the local gate
    // is in the append path.
    if verifier.is_some() {
        return Err("asr-target-speaker-unavailable".to_string());
    }
    state.voice_asr.start(
        input.session_id,
        input.conversation_id,
        input.sample_rate,
        protocol,
        on_event,
    )
}
#[tauri::command]
pub(crate) fn append_voice_asr_audio(
    state: State<'_, AppState>,
    request: Request<'_>,
) -> Result<(), String> {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "asr-packet-format".to_string())
    };
    let session = header(SESSION)?;
    let sequence = header(SEQUENCE)?
        .parse::<u64>()
        .map_err(|_| "asr-packet-sequence".to_string())?;
    let count = header(SAMPLE_COUNT)?
        .parse::<usize>()
        .map_err(|_| "asr-packet-format".to_string())?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("asr-packet-format".to_string());
    };
    state.voice_asr.append(session, sequence, count, bytes)
}
#[tauri::command]
pub(crate) async fn commit_voice_asr_utterance(
    state: State<'_, AppState>,
    input: CommitVoiceAsrUtteranceInput,
) -> Result<(), String> {
    let _reason = input.reason;
    let committed = state.voice_asr.take_commit(&input.session_id)?;
    if committed.bytes.iter().all(|sample| *sample == 0) {
        let _ = committed
            .event
            .send(VoiceAsrStreamEvent::UtteranceDiscarded {
                session_id: committed.session_id,
                utterance_id: committed.utterance_id,
                reason: "no-speech",
            });
        return Ok(());
    }
    let samples = committed
        .bytes
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_767.0)
        .collect::<Vec<_>>();
    let result = crate::voice::session::transcribe_selected_audio(
        &state,
        &samples,
        16_000,
        std::sync::Arc::new(crate::RunCancellation::default()),
    )
    .await;
    match result {
        Ok((text, language)) if !text.trim().is_empty() => {
            let _ = committed.event.send(VoiceAsrStreamEvent::Final {
                session_id: committed.session_id,
                utterance_id: committed.utterance_id,
                revision: 1,
                start_ms: 0,
                end_ms: (samples.len() as u64 * 1_000) / 16_000,
                text,
                language,
            });
        }
        Ok(_) => {
            let _ = committed
                .event
                .send(VoiceAsrStreamEvent::UtteranceDiscarded {
                    session_id: committed.session_id,
                    utterance_id: committed.utterance_id,
                    reason: "no-speech",
                });
        }
        Err(error) if error.starts_with("ASR_NO_SPEECH") => {
            let _ = committed
                .event
                .send(VoiceAsrStreamEvent::UtteranceDiscarded {
                    session_id: committed.session_id,
                    utterance_id: committed.utterance_id,
                    reason: "no-speech",
                });
        }
        Err(error) => {
            let _ = committed.event.send(VoiceAsrStreamEvent::Failed {
                session_id: committed.session_id,
                utterance_id: Some(committed.utterance_id),
                code: "asr-final-timeout",
                message: crate::redact_runtime_text(&error),
                recovery: "Check the ASR provider connection and try again.".to_string(),
                fatal: false,
            });
        }
    }
    Ok(())
}
#[tauri::command]
pub(crate) fn stop_voice_asr_session(
    state: State<'_, AppState>,
    input: StopVoiceAsrSessionInput,
) -> Result<(), String> {
    state
        .voice_asr
        .stop(&input.session_id, input.finalize_current)
}
