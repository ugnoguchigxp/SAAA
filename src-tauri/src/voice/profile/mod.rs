use super::speaker::SpeakerExtractor;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::Zeroizing;

pub(crate) mod streaming_verifier;

const PROFILE_ID: &str = "default";
const KEYCHAIN_SERVICE: &str = "com.saaa.desktop.voice-profile.v1";
const KEYCHAIN_ACCOUNT: &str = "default-master-key-v1";
const CANONICAL_SAMPLE_RATE: u32 = 16_000;
const MIN_SAMPLE_SECONDS: f32 = 10.0;
const MAX_SAMPLE_SECONDS: f32 = 12.0;
const TARGET_SAMPLE_COUNT: usize = 5;
const MIN_READY_SAMPLES: usize = TARGET_SAMPLE_COUNT;
const MIN_READY_DURATION_MS: u64 = 50_000;
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
    #[serde(skip)]
    pub samples: Vec<f32>,
    pub audio_upload_id: String,
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

    #[cfg(any(test, feature = "quality-eval-harness"))]
    pub fn unavailable_for_tests(data_directory: PathBuf) -> Self {
        Self {
            data_directory,
            extractor: None,
            runtime_message: "Unavailable in unit tests".to_string(),
        }
    }

    pub fn snapshot(&self, connection: &Connection) -> Result<VoiceProfileSnapshot, String> {
        update_profile_readiness(connection)?;
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
        mut input: SaveVoiceEnrollmentSampleInput,
    ) -> Result<VoiceProfileSnapshot, String> {
        let input_samples = Zeroizing::new(std::mem::take(&mut input.samples));
        validate_enrollment_input(&input, &input_samples)?;
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
                    decrypt_payload(&master_key, "embedding", &id, &encrypted).and_then(|value| {
                        let value = Zeroizing::new(value);
                        decode_embedding(&value, extractor.dimension())
                    })
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
                    .and_then(|value| {
                        let value = Zeroizing::new(value);
                        decode_embedding(&value, extractor.dimension())
                    })
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

mod codec;
pub use codec::migrate_v10_to_v11;
use codec::*;

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
    fn enrollment_requires_five_samples_of_at_least_ten_seconds() {
        let input = SaveVoiceEnrollmentSampleInput {
            samples: Vec::new(),
            audio_upload_id: "upload_test".to_string(),
            sample_rate: CANONICAL_SAMPLE_RATE,
            input_device_id: "microphone_test".to_string(),
            effective_aec: false,
        };
        assert!(
            validate_enrollment_input(&input, &vec![0.1; CANONICAL_SAMPLE_RATE as usize * 10],)
                .is_ok()
        );
        assert!(validate_enrollment_input(
            &input,
            &vec![0.1; CANONICAL_SAMPLE_RATE as usize * 10 - 1],
        )
        .is_err());

        let connection = Connection::open_in_memory().expect("database opens");
        migrate_v10_to_v11(&connection).expect("migration succeeds");
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','collecting',0,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        for ordinal in 1..=TARGET_SAMPLE_COUNT {
            connection
                .execute(
                    "INSERT INTO voice_profile_samples(
                       id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                       embedding_ciphertext,input_device_id,effective_aec,created_at
                     ) VALUES(?1,'default',?2,?3,10000,16000,?4,'microphone_test',0,'now')",
                    params![
                        format!("sample_{ordinal}"),
                        ordinal as i64,
                        format!("voice-profiles/default/sample_{ordinal}.wav.enc"),
                        vec![0_u8; 64],
                    ],
                )
                .expect("sample inserts");
            update_profile_readiness(&connection).expect("readiness updates");
            let status: String = connection
                .query_row(
                    "SELECT status FROM voice_profiles WHERE id='default'",
                    [],
                    |row| row.get(0),
                )
                .expect("status loads");
            assert_eq!(
                status,
                if ordinal == TARGET_SAMPLE_COUNT {
                    "ready"
                } else {
                    "collecting"
                }
            );
        }
    }

    #[test]
    fn quality_check_accepts_continuous_speech_without_requiring_prompt_completion() {
        let continuous = vec![0.05_f32; CANONICAL_SAMPLE_RATE as usize * 10];
        validate_sample_quality(&continuous).expect("continuous speech is accepted");

        let mut sparse = vec![0.0_f32; CANONICAL_SAMPLE_RATE as usize * 10];
        sparse[..CANONICAL_SAMPLE_RATE as usize * 2].fill(0.05);
        let error = validate_sample_quality(&sparse).expect_err("sparse speech rejects");
        assert!(error.contains("全文を読み切る必要はない"));
        assert!(error.contains("自動停止するまで"));
    }

    #[test]
    fn snapshot_downgrades_legacy_profiles_that_do_not_meet_the_new_minimum() {
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
        let snapshot = runtime.snapshot(&connection).expect("snapshot loads");
        assert_eq!(snapshot.status, "collecting");
        assert!(!snapshot.filter_enabled);
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
