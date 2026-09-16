use std::sync::Arc;

use rusqlite::OptionalExtension;

use super::{
    batch_runtime::{self, BatchDecode},
    speaker_gate_runtime::{PreparedSpeakerScorer, SpeakerScorer},
};
use crate::{
    voice::session::{select_streaming_asr, vad_rms_threshold},
    AppState,
};

pub(crate) struct PreparedSession {
    pub(crate) batch_decoder: Arc<dyn BatchDecode>,
    pub(crate) speaker_scorer: Option<Arc<dyn SpeakerScorer>>,
    pub(crate) vad_threshold: f32,
}

pub(crate) async fn prepare(
    state: &AppState,
    conversation_id: &str,
) -> Result<PreparedSession, String> {
    let (selected, verifier) = state.sqlite_readers.read(|connection| {
        let exists = connection
            .query_row(
                "SELECT 1 FROM conversations WHERE id=?1",
                [conversation_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Database error: {error}"))?
            .is_some();
        if !exists {
            return Err("Conversation does not exist".to_string());
        }
        Ok((
            select_streaming_asr(connection)?,
            state.voice_profile.prepare_streaming_verifier(connection)?,
        ))
    })?;

    let vad_threshold = vad_rms_threshold(&selected.vad_sensitivity);
    let scorer =
        verifier.map(|value| Arc::new(PreparedSpeakerScorer::new(value)) as Arc<dyn SpeakerScorer>);
    Ok(PreparedSession {
        batch_decoder: batch_runtime::decoder(
            state.sqlite_readers.clone(),
            conversation_id.to_string(),
        )?,
        speaker_scorer: scorer,
        vad_threshold,
    })
}
