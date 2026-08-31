use std::collections::BTreeMap;
use std::str::FromStr;

use tauri::{
    ipc::{InvokeBody, Request},
    State,
};

use super::{
    contracts::{
        CommitVoiceAsrUtteranceInput, StartVoiceAsrSessionInput, StopVoiceAsrSessionInput,
        VoiceAsrFailureCode, VoiceAsrStreamEvent,
    },
    native_connection, route,
    session::SessionConfig,
    speaker_gate_runtime::SpeakerGate,
};
use crate::AppState;

const SESSION: &str = "x-saaa-asr-session-id";
const SEQUENCE: &str = "x-saaa-asr-sequence";
const SAMPLE_COUNT: &str = "x-saaa-asr-sample-count";

#[tauri::command]
pub(crate) async fn start_voice_asr_session(
    state: State<'_, AppState>,
    input: StartVoiceAsrSessionInput,
    on_event: tauri::ipc::Channel<VoiceAsrStreamEvent>,
) -> Result<(), String> {
    crate::persistence::audit::record_voice_asr_command(
        &state,
        "asr-session-start-requested",
        &input.session_id,
        Some(&input.conversation_id),
        None,
        None,
        BTreeMap::new(),
    );
    let reservation = state.voice_asr.reserve(
        input.session_id.clone(),
        &input.conversation_id,
        input.sample_rate,
        input.recover_existing.unwrap_or(false),
    )?;
    let cancellation = reservation.cancellation.clone();
    let prepared = match tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err("asr-cancelled".to_string()),
        result = route::prepare(&state, &input.conversation_id) => result,
    } {
        Ok(prepared) => prepared,
        Err(error) => {
            state.voice_asr.abort(reservation);
            return Err(normalize_start_error(&error));
        }
    };
    let current_utterance_id = crate::new_id("voice_asr_utterance");
    let (native, start_degraded) = match prepared.native.as_ref() {
        Some(native_route) => match tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err("asr-cancelled".to_string()),
            result = native_connection::open(
                &native_route.descriptor,
                &input.session_id,
                &current_utterance_id,
                &native_route.model,
                &native_route.language,
            ) => result,
        } {
            Ok(connection) => (Some(connection), None),
            Err(error) if error == "asr-cancelled" => {
                state.voice_asr.abort(reservation);
                return Err(error);
            }
            Err(error) => (None, Some(native_failure_code(&error))),
        },
        None => (None, None),
    };
    let protocol = if native.is_some() {
        "native"
    } else {
        "batch-agreement"
    };
    let speaker_gate = SpeakerGate::new(prepared.speaker_scorer, prepared.vad_threshold);
    let scope = speaker_gate.scope();
    if cancellation.is_cancelled() {
        state.voice_asr.abort(reservation);
        return Err("asr-cancelled".to_string());
    }
    let config = SessionConfig {
        session_id: input.session_id,
        current_utterance_id,
        event: crate::persistence::audit::VoiceAsrAuditChannel::new(
            on_event,
            state.sqlite_writer.clone(),
            input.conversation_id,
        ),
        batch_decoder: prepared.batch_decoder,
        native,
        speaker_gate,
        allowed_languages: prepared.allowed_languages,
        final_timeout: std::time::Duration::from_millis(prepared.timeout_ms.min(15_000)),
        cancellation: reservation.cancellation.clone(),
        start_degraded,
    };
    state
        .voice_asr
        .install(reservation, protocol, scope, config)
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
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| "asr-packet-format".to_string())
    };
    let session_id = header(SESSION)?;
    crate::validate_identifier(session_id, "ASR session id")
        .map_err(|_| "asr-packet-format".to_string())?;
    let sequence = canonical_decimal::<u64>(header(SEQUENCE)?, "asr-packet-sequence")?;
    let sample_count = canonical_decimal::<usize>(header(SAMPLE_COUNT)?, "asr-packet-format")?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("asr-packet-format".to_string());
    };
    state
        .voice_asr
        .append(session_id, sequence, sample_count, bytes)
}

#[tauri::command]
pub(crate) async fn commit_voice_asr_utterance(
    state: State<'_, AppState>,
    input: CommitVoiceAsrUtteranceInput,
) -> Result<(), String> {
    crate::validate_identifier(&input.session_id, "ASR session id")
        .map_err(|_| "asr-session-not-found".to_string())?;
    let reason = match &input.reason {
        super::contracts::CommitReason::Silence => "silence",
        super::contracts::CommitReason::MaxDuration => "max-duration",
    };
    crate::persistence::audit::record_voice_asr_command(
        &state,
        "asr-commit-requested",
        &input.session_id,
        None,
        None,
        None,
        BTreeMap::from([(
            "commitReason".to_string(),
            crate::persistence::audit::AuditAttributeValue::Tag(reason.to_string()),
        )]),
    );
    let result = state
        .voice_asr
        .commit(&input.session_id, input.reason)
        .await;
    crate::persistence::audit::record_voice_asr_command(
        &state,
        "asr-commit-finished",
        &input.session_id,
        None,
        Some(if result.is_ok() { "success" } else { "failure" }),
        result.as_ref().err().map(String::as_str),
        BTreeMap::new(),
    );
    result
}

#[tauri::command]
pub(crate) async fn stop_voice_asr_session(
    state: State<'_, AppState>,
    input: StopVoiceAsrSessionInput,
) -> Result<(), String> {
    crate::validate_identifier(&input.session_id, "ASR session id")
        .map_err(|_| "asr-session-not-found".to_string())?;
    crate::persistence::audit::record_voice_asr_command(
        &state,
        "asr-stop-requested",
        &input.session_id,
        None,
        None,
        None,
        BTreeMap::from([(
            "finalizeCurrent".to_string(),
            crate::persistence::audit::AuditAttributeValue::Boolean(input.finalize_current),
        )]),
    );
    let result = state
        .voice_asr
        .stop(&input.session_id, input.finalize_current)
        .await;
    crate::persistence::audit::record_voice_asr_command(
        &state,
        "asr-stop-finished",
        &input.session_id,
        None,
        Some(if result.is_ok() { "success" } else { "failure" }),
        result.as_ref().err().map(String::as_str),
        BTreeMap::new(),
    );
    result
}

fn canonical_decimal<T>(value: &str, error: &'static str) -> Result<T, String>
where
    T: FromStr + ToString,
{
    let parsed = value.parse::<T>().map_err(|_| error.to_string())?;
    if parsed.to_string() != value {
        return Err(error.to_string());
    }
    Ok(parsed)
}

fn normalize_start_error(error: &str) -> String {
    for code in [
        "asr-target-speaker-unavailable",
        "TARGET_SPEAKER_UNAVAILABLE",
        "asr-language-not-allowed",
        "asr-provider-unavailable",
        "asr-cancelled",
    ] {
        if error.contains(code) {
            return if code == "TARGET_SPEAKER_UNAVAILABLE" {
                "asr-target-speaker-unavailable".to_string()
            } else {
                code.to_string()
            };
        }
    }
    "asr-provider-unavailable".to_string()
}

fn native_failure_code(error: &str) -> VoiceAsrFailureCode {
    if error.contains("protocol") {
        VoiceAsrFailureCode::StreamProtocol
    } else {
        VoiceAsrFailureCode::StreamTimeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_numbers_must_be_canonical_decimal() {
        assert_eq!(canonical_decimal::<u64>("0", "bad"), Ok(0));
        assert_eq!(
            canonical_decimal::<u64>("01", "bad"),
            Err("bad".to_string())
        );
        assert_eq!(
            canonical_decimal::<u64>("+1", "bad"),
            Err("bad".to_string())
        );
    }
}
