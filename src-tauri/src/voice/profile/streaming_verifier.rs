use super::{
    cosine_similarity, database_error, decode_embedding, enrollment_uses_input_device,
    VoiceProfileRuntime, CANONICAL_SAMPLE_RATE, MIN_READY_SAMPLES, MODEL_SHA256, PROFILE_ID,
};
use crate::voice::speaker::SpeakerExtractor;
use rusqlite::{Connection, OptionalExtension};
use zeroize::Zeroizing;

/// Prepared once at session start. The streaming path never reads SQLite or
/// profile files while audio is being processed.
pub(crate) struct PreparedVoiceVerifier {
    extractor: SpeakerExtractor,
    references: Vec<Zeroizing<Vec<f32>>>,
    threshold: f32,
}
impl PreparedVoiceVerifier {
    pub(crate) fn score(&self, samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String> {
        if samples_16k.len() > CANONICAL_SAMPLE_RATE as usize * 2 {
            return Err("TARGET_SPEAKER_REJECTED: speaker window is too long".to_string());
        }
        let candidate = Zeroizing::new(self.extractor.embed(samples_16k.to_vec())?);
        let score = self
            .references
            .iter()
            .map(|reference| cosine_similarity(reference, &candidate))
            .fold(f32::NEG_INFINITY, f32::max);
        if score.is_finite() {
            Ok(score)
        } else {
            Err("TARGET_SPEAKER_REJECTED: speaker score is invalid".to_string())
        }
    }

    pub(crate) fn threshold(&self) -> f32 {
        self.threshold
    }
}
impl VoiceProfileRuntime {
    pub(crate) fn prepare_streaming_verifier(
        &self,
        connection: &Connection,
    ) -> Result<Option<PreparedVoiceVerifier>, String> {
        let profile: Option<(String, bool, f32, String, i64)> = connection.query_row("SELECT status,filter_enabled,threshold,model_sha256,embedding_dimension FROM voice_profiles WHERE id=?1", [PROFILE_ID], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))).optional().map_err(database_error)?;
        let Some((status, filter_enabled, threshold, model_sha256, dimension)) = profile else {
            return Ok(None);
        };
        if !filter_enabled {
            return Ok(None);
        }
        let extractor = self
            .extractor
            .as_ref()
            .ok_or_else(|| format!("TARGET_SPEAKER_UNAVAILABLE: {}", self.runtime_message))?
            .clone();
        if status != "ready"
            || model_sha256 != MODEL_SHA256
            || dimension != extractor.dimension() as i64
        {
            return Err(
                "TARGET_SPEAKER_UNAVAILABLE: The enabled voice profile is not ready or compatible"
                    .to_string(),
            );
        }
        let current_input_device =
            crate::persistence::load_voice_settings(connection)?.input_device_id;
        if !enrollment_uses_input_device(connection, &current_input_device)? {
            return Err(
                "TARGET_SPEAKER_UNAVAILABLE: The voice profile input device does not match the current microphone"
                    .to_string(),
            );
        }
        let stored = connection.prepare("SELECT id,relative_path,embedding FROM voice_profile_samples WHERE profile_id=?1 ORDER BY ordinal").and_then(|mut statement| statement.query_map([PROFILE_ID], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()).map_err(database_error)?;
        if stored.len() < MIN_READY_SAMPLES {
            return Err("TARGET_SPEAKER_UNAVAILABLE: Too few enrollment samples".to_string());
        }
        let references = stored
            .into_iter()
            .map(|(id, path, embedding)| {
                let resolved = self.resolve_sample_path(&id, &path)?;
                if !resolved.is_file() {
                    return Err("TARGET_SPEAKER_UNAVAILABLE: A voice sample is missing".to_string());
                }
                decode_embedding(&embedding, extractor.dimension())
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Some(PreparedVoiceVerifier {
            extractor,
            references,
            threshold,
        }))
    }
}
