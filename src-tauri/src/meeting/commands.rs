use std::sync::Arc;
use zeroize::Zeroize;

use crate::redact::redact_runtime_text;
use crate::{
    database_error, register_active_run, remove_active_run, validate_identifier, AppState,
    RunCancellation,
};

fn publish_state(
    state: &AppState,
    snapshot: &crate::meeting::MeetingSnapshot,
    microphone: crate::situation::contracts::MicrophoneState,
) {
    state
        .meeting
        .emit(crate::meeting::MeetingEvent::StateChanged {
            session_id: snapshot.session_id.clone(),
            state: snapshot.state.clone(),
        });
    state.situation.set_microphone_state(microphone);
}

pub(crate) fn start_meeting_inner(
    state: &AppState,
    input: &crate::meeting::StartInput,
) -> Result<crate::meeting::MeetingSnapshot, String> {
    let _policy = state
        .interaction_policy
        .lock()
        .map_err(|_| "Interaction policy lock unavailable".to_string())?;
    let tts_process = state
        .tts_process
        .lock()
        .map_err(|_| "TTS process lock unavailable".to_string())?;
    if tts_process.is_some() {
        return Err("MEETING_POLICY_TTS_BLOCKED: Stop speech and retry.".to_string());
    }
    drop(tts_process);
    let coding_run_active: bool = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM runtime_runs
                   WHERE route_kind = 'coding.assist' AND status = 'running'
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)
    })?;
    if coding_run_active {
        return Err("MEETING_POLICY_AGENT_BLOCKED: Stop the Coding Agent and retry.".to_string());
    }
    let snapshot = state.meeting.start(input, &state.sqlite_writer)?;
    publish_state(
        state,
        &snapshot,
        crate::situation::contracts::MicrophoneState::SaaaCapturing,
    );
    Ok(snapshot)
}

pub(crate) fn pause_meeting(
    state: &AppState,
    input: crate::meeting::SessionInput,
) -> Result<crate::meeting::MeetingSnapshot, String> {
    let snapshot = state
        .meeting
        .pause(&input.session_id, &state.sqlite_writer)?;
    publish_state(
        state,
        &snapshot,
        crate::situation::contracts::MicrophoneState::Inactive,
    );
    Ok(snapshot)
}

pub(crate) fn resume_meeting(
    state: &AppState,
    input: crate::meeting::SessionInput,
) -> Result<crate::meeting::MeetingSnapshot, String> {
    let snapshot = state
        .meeting
        .resume(&input.session_id, &state.sqlite_writer)?;
    publish_state(
        state,
        &snapshot,
        crate::situation::contracts::MicrophoneState::SaaaCapturing,
    );
    Ok(snapshot)
}

pub(crate) fn stop_meeting(
    state: &AppState,
    input: crate::meeting::SessionInput,
) -> Result<crate::meeting::MeetingSnapshot, String> {
    let snapshot = state
        .meeting
        .stop(&input.session_id, &state.sqlite_writer)?;
    publish_state(
        state,
        &snapshot,
        crate::situation::contracts::MicrophoneState::Inactive,
    );
    Ok(snapshot)
}

pub(crate) async fn append_meeting_audio_segment(
    state: &AppState,
    mut input: crate::meeting::SegmentInput,
) -> Result<crate::meeting::SegmentResult, String> {
    input.samples = state
        .audio_uploads
        .consume(&input.audio_upload_id, "meeting-segment")?;
    let cancellation = Arc::new(RunCancellation::default());
    let appended = state.meeting.append(&mut input, cancellation.clone());
    input.samples.zeroize();
    let samples = appended?;
    state
        .situation
        .set_microphone_state(crate::situation::contracts::MicrophoneState::SaaaTranscribing);
    let transcription = crate::voice::session::transcribe_selected_audio(
        state,
        &samples,
        input.sample_rate,
        cancellation.clone(),
    )
    .await;
    let record_failure = |code: &str, message: String| {
        let message = redact_runtime_text(&message);
        match state
            .meeting
            .fail(&input.session_id, code, &message, &state.sqlite_writer)
        {
            Ok(()) => Err(format!("{code}: {message}")),
            Err(state_error) => Err(format!("{code}: {message}; {state_error}")),
        }
    };
    let result = match transcription {
        Ok((text, language)) => {
            match state.meeting.finish_segment(&input, text, language.clone()) {
                Ok(result) => {
                    state
                        .meeting
                        .emit(crate::meeting::MeetingEvent::TranscriptFinal {
                            session_id: input.session_id.clone(),
                            lane: input.lane.clone(),
                            sequence: input.sequence,
                            text: result.text.clone(),
                            language,
                        });
                    Ok(result)
                }
                Err(error) => {
                    state.meeting.abort_segment(&input);
                    if cancellation.is_cancelled() {
                        Err("Transcription cancelled".to_string())
                    } else {
                        let code = if error == "MEETING_BACKPRESSURE" {
                            "MEETING_BACKPRESSURE"
                        } else {
                            "MEETING_STT_FAILED"
                        };
                        record_failure(code, error)
                    }
                }
            }
        }
        Err(error) => {
            state.meeting.abort_segment(&input);
            if cancellation.is_cancelled() {
                Err("Transcription cancelled".to_string())
            } else if recoverable_asr_rejection(&error) {
                Ok(crate::meeting::SegmentResult {
                    accepted: false,
                    text: String::new(),
                    language: None,
                })
            } else {
                record_failure("MEETING_STT_FAILED", error)
            }
        }
    };
    state.situation.set_microphone_state(
        if state
            .meeting
            .snapshot()
            .is_ok_and(|snapshot| snapshot.state == crate::meeting::MeetingState::Active)
        {
            crate::situation::contracts::MicrophoneState::SaaaCapturing
        } else {
            crate::situation::contracts::MicrophoneState::Inactive
        },
    );
    result
}

fn recoverable_asr_rejection(error: &str) -> bool {
    error.starts_with("ASR_LANGUAGE_NOT_ALLOWED")
        || error.starts_with("ASR_LANGUAGE_UNKNOWN")
        || error.starts_with("ASR_NO_SPEECH")
}

pub(crate) async fn preview_meeting_audio_segment(
    state: &AppState,
    mut input: crate::meeting::PreviewSegmentInput,
) -> Result<(), String> {
    validate_identifier(&input.run_id, "run id")?;
    input.segment.samples = state
        .audio_uploads
        .consume(&input.segment.audio_upload_id, "meeting-segment")?;
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(state, &input.run_id, cancellation.clone())?;
    let preview = state.meeting.preview(&mut input.segment);
    input.segment.samples.zeroize();
    let samples = match preview {
        Ok(preview) => preview,
        Err(error) => {
            remove_active_run(state, &input.run_id);
            return Err(error);
        }
    };
    let transcription = crate::voice::session::transcribe_selected_audio(
        state,
        &samples,
        input.segment.sample_rate,
        cancellation.clone(),
    )
    .await;
    remove_active_run(state, &input.run_id);
    match transcription {
        Ok((text, language)) => {
            if state.meeting.preview_is_current(&input.segment) {
                state
                    .meeting
                    .emit(crate::meeting::MeetingEvent::TranscriptPartial {
                        session_id: input.segment.session_id,
                        lane: input.segment.lane,
                        sequence: input.segment.sequence,
                        text,
                        language,
                    });
            }
            Ok(())
        }
        Err(_) if cancellation.is_cancelled() => Err("Meeting preview cancelled".to_string()),
        Err(error) => Err(redact_runtime_text(&error)),
    }
}

pub(crate) fn save_meeting_transcript(
    state: &AppState,
    input: crate::meeting::SessionInput,
) -> Result<crate::meeting::MeetingSnapshot, String> {
    let snapshot = state
        .meeting
        .save(&input.session_id, &state.sqlite_writer)?;
    publish_state(
        state,
        &snapshot,
        crate::situation::contracts::MicrophoneState::Inactive,
    );
    Ok(snapshot)
}

pub(crate) fn discard_meeting(
    state: &AppState,
    input: crate::meeting::SessionInput,
) -> Result<(), String> {
    state
        .meeting
        .discard(&input.session_id, &state.sqlite_writer)?;
    state
        .meeting
        .emit(crate::meeting::MeetingEvent::StateChanged {
            session_id: None,
            state: crate::meeting::MeetingState::Idle,
        });
    state
        .situation
        .set_microphone_state(crate::situation::contracts::MicrophoneState::Inactive);
    Ok(())
}
