use super::speaker::SpeakerExtractor;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const PROFILE_ID: &str = "default";
const KEYCHAIN_SERVICE: &str = "com.saaa.desktop.voice-profile.v1";
const KEYCHAIN_ACCOUNT: &str = "default-master-key-v1";
const CANONICAL_SAMPLE_RATE: u32 = 16_000;
const MIN_SAMPLE_SECONDS: f32 = 3.0;
const MAX_SAMPLE_SECONDS: f32 = 12.0;
const MIN_READY_SAMPLES: usize = 4;
const TARGET_SAMPLE_COUNT: usize = 5;
const MIN_READY_DURATION_MS: u64 = 20_000;
const DEFAULT_THRESHOLD: f32 = 0.55;
const ENROLLMENT_CONSISTENCY_THRESHOLD: f32 = 0.35;
const ENCRYPTION_MAGIC: &[u8; 8] = b"SAAAENC1";

const MODEL_FILE: &str = "model/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";
const MODEL_SHA256: &str = "f682b514c05d947ee3fa91cd6ec6c5c7543479a128373fa29b1faedccd21fd11";
const LIBRARY_FILE: &str = "lib/libsherpa-onnx-c-api.dylib";
const ONNX_RUNTIME_FILE: &str = "lib/libonnxruntime.dylib";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceProfileSnapshot {
    pub status: String,
    pub filter_enabled: bool,
    pub runtime_available: bool,
    pub runtime_message: String,
    pub sample_count: usize,
    pub target_sample_count: usize,
    pub total_duration_ms: u64,
    pub minimum_duration_ms: u64,
    pub threshold: f32,
    pub samples: Vec<VoiceSampleSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSampleSummary {
    pub id: String,
    pub ordinal: usize,
    pub duration_ms: u64,
    pub input_device_id: String,
    pub effective_aec: bool,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveVoiceEnrollmentSampleInput {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub input_device_id: String,
    pub effective_aec: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetTargetSpeakerFilterInput {
    pub enabled: bool,
}

pub struct VoiceProfileRuntime {
    data_directory: PathBuf,
    extractor: Option<SpeakerExtractor>,
    runtime_message: String,
}

struct NewMasterKeyGuard {
    armed: bool,
}

impl NewMasterKeyGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for NewMasterKeyGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = delete_master_key();
        }
    }
}

impl VoiceProfileRuntime {
    pub fn initialize(resource_directory: PathBuf, data_directory: PathBuf) -> Self {
        let result = (|| {
            verify_artifact(
                &resource_directory.join(MODEL_FILE),
                MODEL_SHA256,
                "speaker model",
            )?;
            verify_bundled_library_exists(
                &resource_directory.join(LIBRARY_FILE),
                "speaker library",
            )?;
            verify_bundled_library_exists(
                &resource_directory.join(ONNX_RUNTIME_FILE),
                "ONNX Runtime library",
            )?;
            SpeakerExtractor::start(
                &resource_directory.join(LIBRARY_FILE),
                &resource_directory.join(MODEL_FILE),
            )
        })();
        match result {
            Ok(extractor) => Self {
                data_directory,
                extractor: Some(extractor),
                runtime_message: "Local speaker verification is ready".to_string(),
            },
            Err(error) => Self {
                data_directory,
                extractor: None,
                runtime_message: error,
            },
        }
    }

    #[cfg(test)]
    pub fn unavailable_for_tests(data_directory: PathBuf) -> Self {
        Self {
            data_directory,
            extractor: None,
            runtime_message: "Unavailable in unit tests".to_string(),
        }
    }

    pub fn snapshot(&self, connection: &Connection) -> Result<VoiceProfileSnapshot, String> {
        let mut snapshot = snapshot_from_connection(
            connection,
            self.extractor.is_some(),
            self.runtime_message.clone(),
        )?;
        if snapshot.sample_count > 0 {
            if let Some(extractor) = self.extractor.as_ref() {
                let metadata: Option<(String, i64)> = connection
                    .query_row(
                        "SELECT model_sha256,embedding_dimension FROM voice_profiles WHERE id=?1",
                        [PROFILE_ID],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(database_error)?;
                if metadata.as_ref().is_some_and(|(model_sha256, dimension)| {
                    model_sha256 != MODEL_SHA256 || *dimension != extractor.dimension() as i64
                }) {
                    snapshot.runtime_available = false;
                    snapshot.runtime_message =
                        "The voice profile was created with an incompatible model. Delete it and enroll again"
                            .to_string();
                }
            }
            if let Err(error) = load_master_key() {
                snapshot.runtime_available = false;
                snapshot.runtime_message = format!("Voice profile key is unavailable: {error}");
            }
            let paths = connection
                .prepare("SELECT id,relative_path FROM voice_profile_samples WHERE profile_id=?1")
                .and_then(|mut statement| {
                    statement
                        .query_map([PROFILE_ID], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(database_error)?;
            if paths.iter().any(|(id, path)| {
                self.resolve_sample_path(id, path)
                    .map_or(true, |path| !path.is_file())
            }) {
                snapshot.runtime_available = false;
                snapshot.runtime_message =
                    "One or more encrypted voice samples are missing. Delete the profile and enroll again"
                        .to_string();
            }
        }
        Ok(snapshot)
    }

    pub fn save_sample(
        &self,
        connection: &Connection,
        input: SaveVoiceEnrollmentSampleInput,
    ) -> Result<VoiceProfileSnapshot, String> {
        validate_enrollment_input(&input)?;
        let extractor = self.extractor.as_ref().ok_or_else(|| {
            format!(
                "Speaker enrollment is unavailable: {}",
                self.runtime_message
            )
        })?;
        let existing: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM voice_profile_samples WHERE profile_id=?1",
                [PROFILE_ID],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if existing as usize >= TARGET_SAMPLE_COUNT {
            return Err(format!(
                "The voice profile already contains the maximum of {TARGET_SAMPLE_COUNT} samples"
            ));
        }
        if existing > 0 {
            let paths = connection
                .prepare("SELECT id,relative_path FROM voice_profile_samples WHERE profile_id=?1")
                .and_then(|mut statement| {
                    statement
                        .query_map([PROFILE_ID], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(database_error)?;
            if paths.iter().any(|(id, path)| {
                self.resolve_sample_path(id, path)
                    .map_or(true, |path| !path.is_file())
            }) {
                return Err(
                    "The existing voice profile is incomplete. Delete it and enroll again"
                        .to_string(),
                );
            }
        }
        let input_samples = Zeroizing::new(input.samples);
        let canonical = Zeroizing::new(resample_mono(
            &input_samples,
            input.sample_rate,
            CANONICAL_SAMPLE_RATE,
        )?);
        validate_sample_quality(&canonical)?;
        let embedding = Zeroizing::new(extractor.embed(canonical.to_vec())?);
        if embedding.len() != extractor.dimension() {
            return Err("Speaker embedding dimension does not match the loaded model".to_string());
        }

        let sample_id = format!("voice_sample_{}", Uuid::new_v4().simple());
        let duration_ms = (canonical.len() as u64 * 1_000) / CANONICAL_SAMPLE_RATE as u64;
        let (master_key, created_master_key) = if existing == 0 {
            load_or_create_master_key()?
        } else {
            (load_master_key()?, false)
        };
        let master_key = Zeroizing::new(master_key);
        let mut master_key_guard = NewMasterKeyGuard {
            armed: created_master_key,
        };
        if existing >= 2 {
            let encrypted_references = connection
                .prepare(
                    "SELECT id,embedding_ciphertext FROM voice_profile_samples
                     WHERE profile_id=?1 ORDER BY ordinal",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map([PROFILE_ID], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(database_error)?;
            let references = encrypted_references
                .into_iter()
                .map(|(id, encrypted)| {
                    decrypt_payload(&master_key, "embedding", &id, &encrypted)
                        .and_then(|value| decode_embedding(&value, extractor.dimension()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let best_score = references
                .iter()
                .map(|reference| cosine_similarity(reference, &embedding))
                .fold(f32::NEG_INFINITY, f32::max);
            if !best_score.is_finite() || best_score < ENROLLMENT_CONSISTENCY_THRESHOLD {
                return Err(
                    "This sample does not match the existing enrollment samples. Record only your own voice and retry"
                        .to_string(),
                );
            }
        }
        let wav = Zeroizing::new(encode_pcm16_wav(&canonical, CANONICAL_SAMPLE_RATE));
        let encrypted_wav = encrypt_payload(&master_key, "audio", &sample_id, &wav)?;
        let encoded_embedding = Zeroizing::new(encode_embedding(&embedding));
        let encrypted_embedding =
            encrypt_payload(&master_key, "embedding", &sample_id, &encoded_embedding)?;
        let relative_path = PathBuf::from("voice-profiles")
            .join(PROFILE_ID)
            .join(format!("{sample_id}.wav.enc"));
        let absolute_path = self.data_directory.join(&relative_path);
        write_private_atomic(&absolute_path, &encrypted_wav)?;

        let created_at = now_iso();
        let transaction = connection.unchecked_transaction().map_err(database_error)?;
        let database_result = (|| {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO voice_profiles(
                       id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                     ) VALUES(?1,'collecting',0,?2,?3,?4,?5,?5)",
                    params![
                        PROFILE_ID,
                        DEFAULT_THRESHOLD,
                        MODEL_SHA256,
                        extractor.dimension() as i64,
                        created_at
                    ],
                )
                .map_err(database_error)?;
            let occupied = transaction
                .prepare("SELECT ordinal FROM voice_profile_samples WHERE profile_id=?1")
                .and_then(|mut statement| {
                    statement
                        .query_map([PROFILE_ID], |row| row.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(database_error)?;
            let ordinal = (1_i64..=TARGET_SAMPLE_COUNT as i64)
                .find(|ordinal| !occupied.contains(ordinal))
                .ok_or_else(|| "No enrollment sample slot is available".to_string())?;
            transaction
                .execute(
                    "INSERT INTO voice_profile_samples(
                       id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                       embedding_ciphertext,input_device_id,effective_aec,created_at
                     ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        sample_id,
                        PROFILE_ID,
                        ordinal,
                        relative_path.to_string_lossy(),
                        duration_ms as i64,
                        CANONICAL_SAMPLE_RATE,
                        encrypted_embedding,
                        input.input_device_id,
                        input.effective_aec,
                        created_at,
                    ],
                )
                .map_err(database_error)?;
            update_profile_readiness(&transaction)?;
            transaction.commit().map_err(database_error)
        })();
        if let Err(error) = database_result {
            let _ = fs::remove_file(&absolute_path);
            return Err(error);
        }
        master_key_guard.disarm();
        self.snapshot(connection)
    }

    pub fn set_filter_enabled(
        &self,
        connection: &Connection,
        enabled: bool,
    ) -> Result<VoiceProfileSnapshot, String> {
        if enabled {
            if self.extractor.is_none() {
                return Err(format!(
                    "Target-speaker filtering is unavailable: {}",
                    self.runtime_message
                ));
            }
            let profile: Option<(String, String, i64)> = connection
                .query_row(
                    "SELECT status,model_sha256,embedding_dimension FROM voice_profiles WHERE id=?1",
                    [PROFILE_ID],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(database_error)?;
            if profile.as_ref().map(|value| value.0.as_str()) != Some("ready") {
                return Err(format!(
                    "Record at least {MIN_READY_SAMPLES} valid samples totaling {MIN_READY_DURATION_MS} ms before enabling the filter"
                ));
            }
            let extractor = self.extractor.as_ref().expect("availability was checked");
            if profile.as_ref().is_some_and(|value| {
                value.1 != MODEL_SHA256 || value.2 != extractor.dimension() as i64
            }) {
                return Err(
                    "The voice profile was created with a different model. Delete it and enroll again"
                        .to_string(),
                );
            }
            load_master_key()?;
        }
        let changed = connection
            .execute(
                "UPDATE voice_profiles SET filter_enabled=?1,updated_at=?2 WHERE id=?3",
                params![enabled, now_iso(), PROFILE_ID],
            )
            .map_err(database_error)?;
        if changed == 0 && enabled {
            return Err("Create a voice profile before enabling the filter".to_string());
        }
        self.snapshot(connection)
    }

    pub fn delete_sample(
        &self,
        connection: &Connection,
        sample_id: &str,
    ) -> Result<VoiceProfileSnapshot, String> {
        validate_sample_id(sample_id)?;
        let relative_path: String = connection
            .query_row(
                "SELECT relative_path FROM voice_profile_samples WHERE id=?1 AND profile_id=?2",
                params![sample_id, PROFILE_ID],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| "Voice sample was not found".to_string())?;
        let absolute_path = self.resolve_sample_path(sample_id, &relative_path)?;
        let transaction = connection.unchecked_transaction().map_err(database_error)?;
        transaction
            .execute(
                "DELETE FROM voice_profile_samples WHERE id=?1 AND profile_id=?2",
                params![sample_id, PROFILE_ID],
            )
            .map_err(database_error)?;
        update_profile_readiness(&transaction)?;
        transaction.commit().map_err(database_error)?;
        match fs::remove_file(absolute_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "The sample metadata was removed, but the encrypted file could not be deleted: {error}"
                ))
            }
        }
        self.snapshot(connection)
    }

    pub fn delete_profile(&self, connection: &Connection) -> Result<VoiceProfileSnapshot, String> {
        let paths = connection
            .prepare("SELECT id,relative_path FROM voice_profile_samples WHERE profile_id=?1")
            .and_then(|mut statement| {
                statement
                    .query_map([PROFILE_ID], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(database_error)?;
        let paths = paths
            .iter()
            .map(|(id, path)| self.resolve_sample_path(id, path))
            .collect::<Result<Vec<_>, _>>()?;
        connection
            .execute("DELETE FROM voice_profiles WHERE id=?1", [PROFILE_ID])
            .map_err(database_error)?;
        let mut deletion_errors = Vec::new();
        for path in paths {
            if let Err(error) = fs::remove_file(self.data_directory.join(path)) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    deletion_errors.push(error.to_string());
                }
            }
        }
        delete_master_key()?;
        if !deletion_errors.is_empty() {
            return Err(format!(
                "The profile key was deleted, but {} encrypted file(s) could not be removed",
                deletion_errors.len()
            ));
        }
        self.snapshot(connection)
    }

    pub fn read_sample(&self, connection: &Connection, sample_id: &str) -> Result<Vec<u8>, String> {
        validate_sample_id(sample_id)?;
        let relative_path: String = connection
            .query_row(
                "SELECT relative_path FROM voice_profile_samples WHERE id=?1 AND profile_id=?2",
                params![sample_id, PROFILE_ID],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| "Voice sample was not found".to_string())?;
        let absolute_path = self.resolve_sample_path(sample_id, &relative_path)?;
        let encrypted = fs::read(absolute_path)
            .map_err(|error| format!("Could not read the encrypted voice sample: {error}"))?;
        let master_key = Zeroizing::new(load_master_key()?);
        decrypt_payload(&master_key, "audio", sample_id, &encrypted)
    }

    /// Verifies every voiced window and fails closed before ASR when filtering is enabled.
    pub fn verify_if_enabled(
        &self,
        connection: &Connection,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<Option<f32>, String> {
        let profile: Option<(String, bool, f32, String, i64)> = connection
            .query_row(
                "SELECT status,filter_enabled,threshold,model_sha256,embedding_dimension
                 FROM voice_profiles WHERE id=?1",
                [PROFILE_ID],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some((status, filter_enabled, threshold, model_sha256, embedding_dimension)) = profile
        else {
            return Ok(None);
        };
        if !filter_enabled {
            return Ok(None);
        }
        if samples.len() > sample_rate as usize * 30 {
            return Err(
                "TARGET_SPEAKER_REJECTED: Filtered utterances are limited to thirty seconds"
                    .to_string(),
            );
        }
        if status != "ready" {
            return Err(
                "TARGET_SPEAKER_UNAVAILABLE: The enabled voice profile is not ready".to_string(),
            );
        }
        let extractor = self
            .extractor
            .as_ref()
            .ok_or_else(|| format!("TARGET_SPEAKER_UNAVAILABLE: {}", self.runtime_message))?;
        if model_sha256 != MODEL_SHA256 || embedding_dimension != extractor.dimension() as i64 {
            return Err(
                "TARGET_SPEAKER_UNAVAILABLE: The voice profile model is incompatible; delete it and enroll again"
                    .to_string(),
            );
        }
        let master_key = Zeroizing::new(
            load_master_key().map_err(|error| format!("TARGET_SPEAKER_UNAVAILABLE: {error}"))?,
        );
        let encrypted_embeddings = connection
            .prepare(
                "SELECT id,relative_path,embedding_ciphertext FROM voice_profile_samples
                 WHERE profile_id=?1 ORDER BY ordinal",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([PROFILE_ID], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(database_error)?;
        if encrypted_embeddings.len() < MIN_READY_SAMPLES {
            return Err("TARGET_SPEAKER_UNAVAILABLE: Too few enrollment samples".to_string());
        }
        let references = encrypted_embeddings
            .into_iter()
            .map(|(id, relative_path, encrypted)| {
                let path = self
                    .resolve_sample_path(&id, &relative_path)
                    .map_err(|error| format!("TARGET_SPEAKER_UNAVAILABLE: {error}"))?;
                if !path.is_file() {
                    return Err(
                        "TARGET_SPEAKER_UNAVAILABLE: An encrypted voice sample is missing"
                            .to_string(),
                    );
                }
                decrypt_payload(&master_key, "embedding", &id, &encrypted)
                    .and_then(|value| decode_embedding(&value, extractor.dimension()))
                    .map_err(|error| format!("TARGET_SPEAKER_UNAVAILABLE: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let canonical = Zeroizing::new(
            resample_mono(samples, sample_rate, CANONICAL_SAMPLE_RATE)
                .map_err(|error| format!("TARGET_SPEAKER_REJECTED: {error}"))?,
        );
        let windows = voiced_windows(&canonical)?;
        let mut minimum_score = 1.0_f32;
        for window in windows {
            let candidate = Zeroizing::new(
                extractor
                    .embed(window)
                    .map_err(|error| format!("TARGET_SPEAKER_REJECTED: {error}"))?,
            );
            let score = references
                .iter()
                .map(|reference| cosine_similarity(reference, &candidate))
                .fold(f32::NEG_INFINITY, f32::max);
            if !score.is_finite() || score < threshold {
                return Err(
                    "TARGET_SPEAKER_REJECTED: The recording does not match the enrolled voice profile"
                        .to_string(),
                );
            }
            minimum_score = minimum_score.min(score);
        }
        Ok(Some(minimum_score))
    }

    fn resolve_sample_path(&self, sample_id: &str, stored_path: &str) -> Result<PathBuf, String> {
        validate_sample_id(sample_id)?;
        let expected = PathBuf::from("voice-profiles")
            .join(PROFILE_ID)
            .join(format!("{sample_id}.wav.enc"));
        if Path::new(stored_path) != expected {
            return Err("Voice sample metadata contains an invalid storage path".to_string());
        }
        Ok(self.data_directory.join(expected))
    }
}

pub fn migrate_v10_to_v11(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 11 {
        return Ok(());
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS voice_profiles (
           id TEXT PRIMARY KEY CHECK(id='default'),
           status TEXT NOT NULL CHECK(status IN ('collecting','ready')),
           filter_enabled INTEGER NOT NULL DEFAULT 0 CHECK(filter_enabled IN (0,1)),
           threshold REAL NOT NULL CHECK(threshold >= 0.0 AND threshold <= 1.0),
           model_sha256 TEXT NOT NULL CHECK(length(model_sha256)=64),
           embedding_dimension INTEGER NOT NULL CHECK(embedding_dimension BETWEEN 1 AND 4096),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS voice_profile_samples (
           id TEXT PRIMARY KEY,
           profile_id TEXT NOT NULL,
           ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 5),
           relative_path TEXT NOT NULL UNIQUE CHECK(length(relative_path) BETWEEN 1 AND 500),
           duration_ms INTEGER NOT NULL CHECK(duration_ms BETWEEN 3000 AND 12000),
           sample_rate INTEGER NOT NULL CHECK(sample_rate=16000),
           embedding_ciphertext BLOB NOT NULL CHECK(length(embedding_ciphertext) BETWEEN 64 AND 65536),
           input_device_id TEXT NOT NULL CHECK(length(input_device_id) BETWEEN 1 AND 300),
           effective_aec INTEGER NOT NULL CHECK(effective_aec IN (0,1)),
           created_at TEXT NOT NULL,
           FOREIGN KEY(profile_id) REFERENCES voice_profiles(id) ON DELETE CASCADE,
           UNIQUE(profile_id,ordinal)
         );
         CREATE INDEX IF NOT EXISTS idx_voice_profile_samples_profile_ordinal
           ON voice_profile_samples(profile_id,ordinal);",
    )?;
    Ok(())
}

fn snapshot_from_connection(
    connection: &Connection,
    runtime_available: bool,
    runtime_message: String,
) -> Result<VoiceProfileSnapshot, String> {
    let profile: Option<(String, bool, f32)> = connection
        .query_row(
            "SELECT status,filter_enabled,threshold FROM voice_profiles WHERE id=?1",
            [PROFILE_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database_error)?;
    let samples = connection
        .prepare(
            "SELECT id,ordinal,duration_ms,input_device_id,effective_aec,created_at
             FROM voice_profile_samples WHERE profile_id=?1 ORDER BY ordinal",
        )
        .and_then(|mut statement| {
            statement
                .query_map([PROFILE_ID], |row| {
                    Ok(VoiceSampleSummary {
                        id: row.get(0)?,
                        ordinal: row.get::<_, i64>(1)? as usize,
                        duration_ms: row.get::<_, i64>(2)? as u64,
                        input_device_id: row.get(3)?,
                        effective_aec: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(database_error)?;
    let total_duration_ms = samples.iter().map(|sample| sample.duration_ms).sum();
    let (status, filter_enabled, threshold) =
        profile.unwrap_or_else(|| ("empty".to_string(), false, DEFAULT_THRESHOLD));
    Ok(VoiceProfileSnapshot {
        status,
        filter_enabled,
        runtime_available,
        runtime_message,
        sample_count: samples.len(),
        target_sample_count: TARGET_SAMPLE_COUNT,
        total_duration_ms,
        minimum_duration_ms: MIN_READY_DURATION_MS,
        threshold,
        samples,
    })
}

fn update_profile_readiness(connection: &Connection) -> Result<(), String> {
    let (count, duration): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(duration_ms),0) FROM voice_profile_samples WHERE profile_id=?1",
            [PROFILE_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error)?;
    let ready = count as usize >= MIN_READY_SAMPLES && duration as u64 >= MIN_READY_DURATION_MS;
    connection
        .execute(
            "UPDATE voice_profiles
             SET status=?1,filter_enabled=CASE WHEN ?1='ready' THEN filter_enabled ELSE 0 END,updated_at=?2
             WHERE id=?3",
            params![if ready { "ready" } else { "collecting" }, now_iso(), PROFILE_ID],
        )
        .map_err(database_error)?;
    Ok(())
}

fn validate_enrollment_input(input: &SaveVoiceEnrollmentSampleInput) -> Result<(), String> {
    if !(8_000..=192_000).contains(&input.sample_rate)
        || input.samples.is_empty()
        || input.samples.iter().any(|sample| !sample.is_finite())
    {
        return Err("Enrollment audio is empty or invalid".to_string());
    }
    let duration = input.samples.len() as f32 / input.sample_rate as f32;
    if !(MIN_SAMPLE_SECONDS..=MAX_SAMPLE_SECONDS).contains(&duration) {
        return Err("Each voice sample must be between 3 and 12 seconds".to_string());
    }
    if input.input_device_id.trim().is_empty() || input.input_device_id.len() > 300 {
        return Err("Enrollment input device is invalid".to_string());
    }
    Ok(())
}

fn validate_sample_quality(samples: &[f32]) -> Result<(), String> {
    let rms = root_mean_square(samples);
    let clipped = samples
        .iter()
        .filter(|sample| sample.abs() >= 0.985)
        .count() as f32
        / samples.len() as f32;
    if rms < 0.008 {
        return Err("The sample is too quiet. Move closer to the microphone and retry".to_string());
    }
    if clipped > 0.02 {
        return Err("The sample is clipped. Lower the input level and retry".to_string());
    }
    let frame_size = CANONICAL_SAMPLE_RATE as usize / 50;
    let voiced_threshold = (rms * 0.35).max(0.008);
    let voiced_frames = samples
        .chunks(frame_size)
        .filter(|frame| frame.len() == frame_size && root_mean_square(frame) >= voiced_threshold)
        .count();
    let total_frames = samples.len() / frame_size;
    if total_frames == 0 || voiced_frames as f32 / (total_frames as f32) < 0.4 {
        return Err(
            "The sample contains too little speech. Read the full prompt without long pauses"
                .to_string(),
        );
    }
    Ok(())
}

fn voiced_windows(samples: &[f32]) -> Result<Vec<Vec<f32>>, String> {
    if samples.len() < CANONICAL_SAMPLE_RATE as usize {
        return Err(
            "TARGET_SPEAKER_REJECTED: At least one second of speech is required".to_string(),
        );
    }
    let window_size = CANONICAL_SAMPLE_RATE as usize * 2;
    let hop_size = CANONICAL_SAMPLE_RATE as usize;
    if samples.len() <= window_size {
        if root_mean_square(samples) < 0.006 {
            return Err("TARGET_SPEAKER_REJECTED: No usable speech was detected".to_string());
        }
        return Ok(vec![samples.to_vec()]);
    }
    let mut windows = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        let end = (start + window_size).min(samples.len());
        if end - start < CANONICAL_SAMPLE_RATE as usize {
            break;
        }
        let window = &samples[start..end];
        if root_mean_square(window) >= 0.006 {
            windows.push(window.to_vec());
        }
        if end == samples.len() {
            break;
        }
        start += hop_size;
    }
    if windows.is_empty() {
        return Err("TARGET_SPEAKER_REJECTED: No usable speech was detected".to_string());
    }
    Ok(windows)
}

fn resample_mono(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>, String> {
    if from_rate == 0 || to_rate == 0 || samples.is_empty() {
        return Err("Audio sample rate is invalid".to_string());
    }
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    let output_len = ((samples.len() as u64 * to_rate as u64) / from_rate as u64) as usize;
    if output_len == 0 {
        return Err("Audio is too short to resample".to_string());
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let mut output = Vec::with_capacity(output_len);
    for index in 0..output_len {
        let position = index as f64 * ratio;
        let lower = position.floor() as usize;
        let upper = (lower + 1).min(samples.len() - 1);
        let fraction = (position - lower as f64) as f32;
        output.push(samples[lower] * (1.0 - fraction) + samples[upper] * fraction);
    }
    Ok(output)
}

fn encode_pcm16_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        wav.extend_from_slice(&value.to_le_bytes());
    }
    wav
}

fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + embedding.len() * 4);
    bytes.extend_from_slice(&(embedding.len() as u32).to_le_bytes());
    for value in embedding {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_embedding(
    bytes: &[u8],
    expected_dimension: usize,
) -> Result<Zeroizing<Vec<f32>>, String> {
    if bytes.len() < 4 {
        return Err("Encrypted speaker embedding is truncated".to_string());
    }
    let dimension = u32::from_le_bytes(bytes[..4].try_into().expect("four-byte slice")) as usize;
    if dimension != expected_dimension || bytes.len() != 4 + dimension * 4 {
        return Err("Encrypted speaker embedding has an incompatible dimension".to_string());
    }
    let values = bytes[4..]
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four-byte chunk")))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err("Encrypted speaker embedding contains invalid values".to_string());
    }
    Ok(Zeroizing::new(values))
}

fn encrypt_payload(
    master: &[u8; 32],
    kind: &str,
    id: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    let key = Zeroizing::new(derive_key(master, kind, id)?);
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|_| "Could not initialize voice-profile encryption".to_string())?;
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let aad = format!("saaa-voice-profile-v1:{kind}:{id}");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "Could not encrypt voice-profile data".to_string())?;
    let mut output =
        Vec::with_capacity(ENCRYPTION_MAGIC.len() + nonce_bytes.len() + ciphertext.len());
    output.extend_from_slice(ENCRYPTION_MAGIC);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

fn decrypt_payload(
    master: &[u8; 32],
    kind: &str,
    id: &str,
    encrypted: &[u8],
) -> Result<Vec<u8>, String> {
    if encrypted.len() < ENCRYPTION_MAGIC.len() + 12 + 16
        || &encrypted[..ENCRYPTION_MAGIC.len()] != ENCRYPTION_MAGIC
    {
        return Err("Encrypted voice-profile data is invalid".to_string());
    }
    let key = Zeroizing::new(derive_key(master, kind, id)?);
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|_| "Could not initialize voice-profile decryption".to_string())?;
    let nonce_start = ENCRYPTION_MAGIC.len();
    let nonce_end = nonce_start + 12;
    let aad = format!("saaa-voice-profile-v1:{kind}:{id}");
    cipher
        .decrypt(
            Nonce::from_slice(&encrypted[nonce_start..nonce_end]),
            Payload {
                msg: &encrypted[nonce_end..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "Voice-profile data could not be authenticated or decrypted".to_string())
}

fn derive_key(master: &[u8; 32], kind: &str, id: &str) -> Result<[u8; 32], String> {
    let hkdf = Hkdf::<Sha256>::new(Some(PROFILE_ID.as_bytes()), master);
    let mut key = [0_u8; 32];
    hkdf.expand(format!("saaa:{kind}:{id}").as_bytes(), &mut key)
        .map_err(|_| "Could not derive a voice-profile encryption key".to_string())?;
    Ok(key)
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return f32::NEG_INFINITY;
    }
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for (left, right) in left.iter().zip(right) {
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator <= f32::EPSILON {
        f32::NEG_INFINITY
    } else {
        dot / denominator
    }
}

fn root_mean_square(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn verify_artifact(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|_| format!("The bundled {label} is missing"))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        return Err(format!("The bundled {label} failed its integrity check"));
    }
    Ok(())
}

fn verify_bundled_library_exists(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|_| format!("The bundled {label} is missing"))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(format!("The bundled {label} is invalid"));
    }
    // The install script verifies the upstream archive. Production signing may
    // rewrite Mach-O signature bytes, so runtime verification uses the enclosing
    // app signature plus successful native symbol loading for dylibs.
    Ok(())
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Voice sample path has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the voice-profile directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not protect the voice-profile directory: {error}"))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("Could not create the encrypted voice sample: {error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Could not persist the encrypted voice sample: {error}"
        ));
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Could not finalize the encrypted voice sample: {error}")
    })
}

fn validate_sample_id(sample_id: &str) -> Result<(), String> {
    if !sample_id.starts_with("voice_sample_")
        || sample_id.len() > 80
        || !sample_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("Voice sample id is invalid".to_string());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn load_or_create_master_key() -> Result<([u8; 32], bool), String> {
    use security_framework::passwords::{generic_password, PasswordOptions};
    match generic_password(PasswordOptions::new_generic_password(
        KEYCHAIN_SERVICE,
        KEYCHAIN_ACCOUNT,
    )) {
        Ok(value) => value
            .try_into()
            .map(|key| (key, false))
            .map_err(|_| "The Keychain voice-profile key has an invalid length".to_string()),
        Err(error) if error.code() == -25_300 => {
            let mut key = [0_u8; 32];
            OsRng.fill_bytes(&mut key);
            security_framework::passwords::set_generic_password(
                KEYCHAIN_SERVICE,
                KEYCHAIN_ACCOUNT,
                &key,
            )
            .map_err(|error| {
                format!("Could not store the voice-profile key in Keychain: {error}")
            })?;
            Ok((key, true))
        }
        Err(error) => Err(format!(
            "Could not read the voice-profile key from Keychain: {error}"
        )),
    }
}

#[cfg(not(target_os = "macos"))]
fn load_or_create_master_key() -> Result<([u8; 32], bool), String> {
    Err("Voice-profile encryption requires macOS Keychain".to_string())
}

#[cfg(target_os = "macos")]
fn load_master_key() -> Result<[u8; 32], String> {
    use security_framework::passwords::{generic_password, PasswordOptions};
    let value = generic_password(PasswordOptions::new_generic_password(
        KEYCHAIN_SERVICE,
        KEYCHAIN_ACCOUNT,
    ))
    .map_err(|error| format!("Could not read the voice-profile key from Keychain: {error}"))?;
    value
        .try_into()
        .map_err(|_| "The Keychain voice-profile key has an invalid length".to_string())
}

#[cfg(not(target_os = "macos"))]
fn load_master_key() -> Result<[u8; 32], String> {
    Err("Voice-profile encryption requires macOS Keychain".to_string())
}

#[cfg(target_os = "macos")]
fn delete_master_key() -> Result<(), String> {
    match security_framework::passwords::delete_generic_password(
        KEYCHAIN_SERVICE,
        KEYCHAIN_ACCOUNT,
    ) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25_300 => Ok(()),
        Err(error) => Err(format!(
            "Voice files were removed, but the profile key could not be deleted from Keychain: {error}"
        )),
    }
}

#[cfg(not(target_os = "macos"))]
fn delete_master_key() -> Result<(), String> {
    Err("Voice-profile encryption requires macOS Keychain".to_string())
}

fn database_error(error: rusqlite::Error) -> String {
    format!("Voice-profile database operation failed: {error}")
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_round_trip_authenticates_kind_and_id() {
        let key = [7_u8; 32];
        let encrypted =
            encrypt_payload(&key, "audio", "sample_one", b"private voice").expect("encrypts");
        assert_ne!(
            encrypted
                .windows(13)
                .find(|window| *window == b"private voice"),
            Some(b"private voice".as_slice())
        );
        assert_eq!(
            decrypt_payload(&key, "audio", "sample_one", &encrypted).expect("decrypts"),
            b"private voice"
        );
        assert!(decrypt_payload(&key, "embedding", "sample_one", &encrypted).is_err());
        let mut tampered = encrypted;
        *tampered.last_mut().expect("ciphertext exists") ^= 1;
        assert!(decrypt_payload(&key, "audio", "sample_one", &tampered).is_err());
    }

    #[test]
    fn resampling_and_wav_encoding_produce_canonical_audio() {
        let input = vec![0.25_f32; 48_000];
        let resampled = resample_mono(&input, 48_000, 16_000).expect("resamples");
        assert_eq!(resampled.len(), 16_000);
        let wav = encode_pcm16_wav(&resampled, 16_000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 32_000);
    }

    #[test]
    fn voice_profile_migration_is_idempotent_and_defaults_to_fail_safe() {
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_v10_to_v11(&connection).expect("migration succeeds");
        migrate_v10_to_v11(&connection).expect("migration repeats");
        let runtime = VoiceProfileRuntime::unavailable_for_tests(PathBuf::new());
        let snapshot = runtime.snapshot(&connection).expect("snapshot loads");
        assert_eq!(snapshot.status, "empty");
        assert!(!snapshot.filter_enabled);
        assert!(!snapshot.runtime_available);
        let metadata_columns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('voice_profiles')
                 WHERE name IN ('model_sha256','embedding_dimension')",
                [],
                |row| row.get(0),
            )
            .expect("metadata columns load");
        assert_eq!(metadata_columns, 2);
    }

    #[test]
    fn enabled_filter_rejects_unbounded_audio_before_model_or_asr() {
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_v10_to_v11(&connection).expect("migration succeeds");
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','ready',1,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        let runtime = VoiceProfileRuntime::unavailable_for_tests(PathBuf::new());
        let error = runtime
            .verify_if_enabled(&connection, &vec![0.1; 16_000 * 31], 16_000)
            .expect_err("long audio rejects");
        assert!(error.starts_with("TARGET_SPEAKER_REJECTED"));
    }

    #[test]
    fn cosine_similarity_handles_matching_and_empty_vectors() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 0.0001);
        assert_eq!(cosine_similarity(&[], &[]), f32::NEG_INFINITY);
    }

    #[test]
    fn stored_sample_paths_cannot_escape_the_voice_profile_directory() {
        let runtime = VoiceProfileRuntime::unavailable_for_tests(PathBuf::from("/private/data"));
        let sample_id = "voice_sample_0123456789abcdef";
        assert!(runtime
            .resolve_sample_path(sample_id, "../../Documents/private.txt")
            .is_err());
        assert_eq!(
            runtime
                .resolve_sample_path(
                    sample_id,
                    "voice-profiles/default/voice_sample_0123456789abcdef.wav.enc",
                )
                .expect("generated path is accepted"),
            PathBuf::from(
                "/private/data/voice-profiles/default/voice_sample_0123456789abcdef.wav.enc"
            )
        );
    }
}
