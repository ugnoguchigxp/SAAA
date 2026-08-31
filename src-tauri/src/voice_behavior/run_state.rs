use crate::AppState;

use super::{effective_presentation, RunSpeechOverride};

pub(super) fn apply_ui_speech_runtime(
    state: &AppState,
    conversation_id: &str,
    speech_output: &str,
) -> Result<(), String> {
    let override_value = match speech_output {
        "muted" => RunSpeechOverride::Silent,
        "inherit" => RunSpeechOverride::Speak,
        _ => return Err("Voice policy value is invalid".to_string()),
    };
    let run_ids = {
        let mut runs = state
            .voice_behavior
            .runs
            .lock()
            .map_err(|_| "Voice behavior runtime lock unavailable".to_string())?;
        runs.iter_mut()
            .filter_map(|(run_id, run)| {
                if run.conversation_id != conversation_id {
                    return None;
                }
                if override_value == RunSpeechOverride::Silent
                    || run.speech_override != Some(RunSpeechOverride::Silent)
                {
                    run.speech_override = Some(override_value);
                }
                Some(run_id.clone())
            })
            .collect::<Vec<_>>()
    };
    for run_id in run_ids {
        let speak = effective_presentation(state, Some(&run_id), conversation_id)
            .map(|presentation| presentation.decision == "speak")
            .unwrap_or(false);
        state.streaming_tts.set_enabled(&run_id, speak);
    }
    Ok(())
}
