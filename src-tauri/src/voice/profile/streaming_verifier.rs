use super::{
    cosine_similarity, database_error, decode_embedding, decrypt_payload, load_master_key,
    VoiceProfileRuntime, CANONICAL_SAMPLE_RATE, MIN_READY_SAMPLES, MODEL_SHA256, PROFILE_ID,
};
use crate::voice::speaker::SpeakerExtractor;
use rusqlite::{Connection, OptionalExtension};
use zeroize::Zeroizing;

/// Prepared once at session start. The streaming path never reads SQLite,
/// Keychain, or profile files while audio is being processed.
#[allow(dead_code)]
pub(crate) struct PreparedVoiceVerifier {
    extractor: SpeakerExtractor,
    references: Vec<Zeroizing<Vec<f32>>>,
    threshold: f32,
}
#[allow(dead_code)]
impl PreparedVoiceVerifier {
    pub(crate) fn score(&self, samples_16k: Vec<f32>) -> Result<f32, String> {
        if samples_16k.len() > CANONICAL_SAMPLE_RATE as usize * 2 {
            return Err("TARGET_SPEAKER_REJECTED: speaker window is too long".to_string());
        }
        let candidate = Zeroizing::new(self.extractor.embed(samples_16k)?);
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
        let key = Zeroizing::new(
            load_master_key().map_err(|error| format!("TARGET_SPEAKER_UNAVAILABLE: {error}"))?,
        );
        let encrypted = connection.prepare("SELECT id,relative_path,embedding_ciphertext FROM voice_profile_samples WHERE profile_id=?1 ORDER BY ordinal").and_then(|mut statement| statement.query_map([PROFILE_ID], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()).map_err(database_error)?;
        if encrypted.len() < MIN_READY_SAMPLES {
            return Err("TARGET_SPEAKER_UNAVAILABLE: Too few enrollment samples".to_string());
        }
        let references = encrypted
            .into_iter()
            .map(|(id, path, ciphertext)| {
                let resolved = self.resolve_sample_path(&id, &path)?;
                if !resolved.is_file() {
                    return Err(
                        "TARGET_SPEAKER_UNAVAILABLE: An encrypted voice sample is missing"
                            .to_string(),
                    );
                }
                let plain = Zeroizing::new(decrypt_payload(&key, "embedding", &id, &ciphertext)?);
                decode_embedding(&plain, extractor.dimension())
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Some(PreparedVoiceVerifier {
            extractor,
            references,
            threshold,
        }))
    }
}
