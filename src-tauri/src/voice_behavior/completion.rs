use crate::{AppState, VoiceRuntimeSettings};

use super::{
    persistence::{load_policy, snapshot_from, PolicyRow},
    ConversationVoicePolicySnapshot, RunSpeechOverride, VoicePresentationDecision,
};

pub(crate) fn effective_presentation(
    state: &AppState,
    run_id: Option<&str>,
    conversation_id: &str,
) -> Result<VoicePresentationDecision, String> {
    resolved_presentation(state, run_id, conversation_id).map(|resolved| resolved.0)
}

pub(crate) fn presentation_and_snapshot(
    state: &AppState,
    run_id: Option<&str>,
    conversation_id: &str,
) -> Result<(VoicePresentationDecision, ConversationVoicePolicySnapshot), String> {
    let (presentation, policy, voice, meeting_blocked) =
        resolved_presentation(state, run_id, conversation_id)?;
    let snapshot = snapshot_from(meeting_blocked, policy, voice);
    Ok((presentation, snapshot))
}

fn resolved_presentation(
    state: &AppState,
    run_id: Option<&str>,
    conversation_id: &str,
) -> Result<
    (
        VoicePresentationDecision,
        PolicyRow,
        VoiceRuntimeSettings,
        bool,
    ),
    String,
> {
    let run_override = match run_id {
        Some(run_id) => {
            let runs = state
                .voice_behavior
                .runs
                .lock()
                .map_err(|_| "Voice behavior runtime lock unavailable".to_string())?;
            let run = runs
                .get(run_id)
                .ok_or_else(|| "Voice behavior state is not available for this run".to_string())?;
            if run.conversation_id != conversation_id {
                return Err(
                    "Voice behavior run context does not match the conversation".to_string()
                );
            }
            run.speech_override
        }
        None => None,
    };
    let (policy, voice) = state.sqlite_readers.read(|connection| {
        Ok((
            load_policy(connection, conversation_id)?,
            crate::persistence::load_voice_settings(connection)?,
        ))
    })?;
    let meeting_blocked = state.meeting.blocks_tts();
    let presentation = effective_presentation_from(
        meeting_blocked,
        voice.auto_speak,
        run_override,
        &policy.speech_output_override,
    );
    Ok((presentation, policy, voice, meeting_blocked))
}

pub(crate) fn completion_state(
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
) -> (
    VoicePresentationDecision,
    Option<Box<ConversationVoicePolicySnapshot>>,
) {
    match presentation_and_snapshot(state, Some(run_id), conversation_id) {
        Ok((presentation, snapshot)) => (presentation, Some(Box::new(snapshot))),
        Err(_) => (
            VoicePresentationDecision {
                decision: "silent".to_string(),
                reason_code: "route_blocked".to_string(),
            },
            None,
        ),
    }
}

pub(crate) fn upper_policies_allow_speech(state: &AppState) -> Result<bool, String> {
    if state.meeting.blocks_tts() {
        return Ok(false);
    }
    state
        .sqlite_readers
        .read(|connection| Ok(crate::persistence::load_voice_settings(connection)?.auto_speak))
}

pub(super) fn effective_presentation_from(
    meeting_blocked: bool,
    auto_speak: bool,
    run_override: Option<RunSpeechOverride>,
    conversation_override: &str,
) -> VoicePresentationDecision {
    let (decision, reason_code) = if meeting_blocked {
        ("silent", "meeting_blocked")
    } else if !auto_speak {
        ("silent", "global_opt_out")
    } else if run_override == Some(RunSpeechOverride::Silent) {
        ("silent", "turn_override")
    } else if run_override == Some(RunSpeechOverride::Speak) {
        ("speak", "turn_override")
    } else if conversation_override == "muted" {
        ("silent", "conversation_override")
    } else {
        ("speak", "global_default")
    };
    VoicePresentationDecision {
        decision: decision.to_string(),
        reason_code: reason_code.to_string(),
    }
}
