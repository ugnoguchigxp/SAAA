use super::speaker::SpeakerExtractor;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;

use crate::persistence::{SqliteReaders, SqliteWriter};

mod enrollment;
pub(crate) mod streaming_verifier;

const PROFILE_ID: &str = "default";
const CANONICAL_SAMPLE_RATE: u32 = 16_000;
const MIN_SAMPLE_SECONDS: f32 = 10.0;
const MAX_SAMPLE_SECONDS: f32 = 12.0;
const TARGET_SAMPLE_COUNT: usize = 5;
const MIN_READY_SAMPLES: usize = TARGET_SAMPLE_COUNT;
const MIN_READY_DURATION_MS: u64 = 50_000;
const DEFAULT_THRESHOLD: f32 = 0.55;
const ENROLLMENT_CONSISTENCY_THRESHOLD: f32 = 0.35;

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
    mutation: Mutex<()>,
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
                mutation: Mutex::new(()),
            },
            Err(error) => Self {
                data_directory,
                extractor: None,
                runtime_message: error,
                mutation: Mutex::new(()),
            },
        }
    }

    #[cfg(any(test, feature = "quality-eval-harness"))]
    pub fn unavailable_for_tests(data_directory: PathBuf) -> Self {
        Self {
            data_directory,
            extractor: None,
            runtime_message: "Unavailable in unit tests".to_string(),
            mutation: Mutex::new(()),
        }
    }

    fn with_mutation<T>(&self, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "Voice profile mutation lock unavailable".to_string())?;
        operation()
    }

    pub(crate) fn set_filter_enabled(
        &self,
        writer: &SqliteWriter,
        enabled: bool,
    ) -> Result<VoiceProfileSnapshot, String> {
        self.with_mutation(|| {
            writer.write(|connection| self.set_filter_enabled_in_connection(connection, enabled))
        })
    }

    pub(crate) fn delete_sample(
        &self,
        writer: &SqliteWriter,
        sample_id: &str,
    ) -> Result<VoiceProfileSnapshot, String> {
        self.with_mutation(|| {
            writer.write(|connection| self.delete_sample_from_connection(connection, sample_id))
        })
    }

    pub(crate) fn delete_profile(
        &self,
        writer: &SqliteWriter,
    ) -> Result<VoiceProfileSnapshot, String> {
        self.with_mutation(|| {
            writer.write(|connection| self.delete_profile_from_connection(connection))
        })
    }

    pub(crate) fn read_with_snapshot<T>(
        &self,
        readers: &SqliteReaders,
        operation: impl FnOnce(&Connection, VoiceProfileSnapshot) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_mutation(|| {
            readers.read(|connection| {
                let snapshot = self.snapshot(connection)?;
                operation(connection, snapshot)
            })
        })
    }

    fn snapshot(&self, connection: &Connection) -> Result<VoiceProfileSnapshot, String> {
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
                    "One or more voice samples are missing. Delete the profile and enroll again"
                        .to_string();
            }
        }
        Ok(snapshot)
    }

    pub(crate) fn reconcile_readiness(&self, connection: &Connection) -> Result<(), String> {
        update_profile_readiness(connection)
    }

    fn set_filter_enabled_in_connection(
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
            let current_input_device =
                crate::persistence::load_voice_settings(connection)?.input_device_id;
            if !enrollment_uses_input_device(connection, &current_input_device)? {
                return Err(
                    "The voice profile was recorded with a different input device. Re-enroll before enabling target-speaker filtering"
                        .to_string(),
                );
            }
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

    fn delete_sample_from_connection(
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
        match fs::remove_file(&absolute_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "The voice file could not be deleted; sample metadata was retained for retry: {error}"
                ))
            }
        }
        let transaction = connection.unchecked_transaction().map_err(database_error)?;
        transaction
            .execute(
                "DELETE FROM voice_profile_samples WHERE id=?1 AND profile_id=?2",
                params![sample_id, PROFILE_ID],
            )
            .map_err(database_error)?;
        update_profile_readiness(&transaction)?;
        transaction.commit().map_err(database_error)?;
        self.snapshot(connection)
    }

    fn delete_profile_from_connection(
        &self,
        connection: &Connection,
    ) -> Result<VoiceProfileSnapshot, String> {
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
        let mut deletion_errors = Vec::new();
        for path in &paths {
            if let Err(error) = fs::remove_file(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    deletion_errors.push(error.to_string());
                }
            }
        }
        if !deletion_errors.is_empty() {
            return Err(format!(
                "{} voice file(s) could not be removed; profile metadata was retained for retry",
                deletion_errors.len()
            ));
        }
        connection
            .execute("DELETE FROM voice_profiles WHERE id=?1", [PROFILE_ID])
            .map_err(database_error)?;
        self.snapshot(connection)
    }

    pub(crate) fn read_sample(
        &self,
        readers: &SqliteReaders,
        sample_id: &str,
    ) -> Result<Vec<u8>, String> {
        self.with_mutation(|| {
            readers.read(|connection| self.read_sample_from_connection(connection, sample_id))
        })
    }

    fn read_sample_from_connection(
        &self,
        connection: &Connection,
        sample_id: &str,
    ) -> Result<Vec<u8>, String> {
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
        fs::read(absolute_path).map_err(|error| format!("Could not read the voice sample: {error}"))
    }

    fn resolve_sample_path(&self, sample_id: &str, stored_path: &str) -> Result<PathBuf, String> {
        let expected = expected_sample_relative_path(sample_id)?;
        if Path::new(stored_path) != expected {
            return Err("Voice sample metadata contains an invalid storage path".to_string());
        }
        Ok(self.data_directory.join(expected))
    }
}

fn enrollment_uses_input_device(
    connection: &Connection,
    input_device_id: &str,
) -> Result<bool, String> {
    let (total, matching): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN input_device_id=?1 THEN 1 ELSE 0 END),0)
             FROM voice_profile_samples WHERE profile_id=?2",
            params![input_device_id, PROFILE_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error)?;
    Ok(total as usize >= MIN_READY_SAMPLES && matching == total)
}

mod codec;
use codec::*;
pub use codec::{migrate_v10_to_v11, migrate_v14_to_v15, reconcile_voice_profile_storage};

#[cfg(test)]
mod tests {
    use super::*;

    fn migrate_plain_voice_profile_schema(connection: &Connection) {
        migrate_v10_to_v11(connection).expect("legacy migration succeeds");
        let transaction = connection
            .unchecked_transaction()
            .expect("plaintext migration starts");
        migrate_v14_to_v15(&transaction).expect("plaintext migration succeeds");
        transaction.commit().expect("plaintext migration commits");
    }

    #[test]
    fn embedding_codec_round_trips_the_expected_dimension() {
        let embedding = [0.25_f32, -0.5, 1.0];
        let encoded = encode_embedding(&embedding);
        let decoded = decode_embedding(&encoded, embedding.len()).expect("embedding decodes");
        assert_eq!(decoded.as_slice(), embedding);
        assert!(decode_embedding(&encoded, embedding.len() + 1).is_err());
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
        migrate_v10_to_v11(&connection).expect("legacy migration succeeds");
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','ready',1,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("legacy profile inserts");
        connection
            .execute(
                "INSERT INTO voice_profile_samples(
                   id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                   embedding_ciphertext,input_device_id,effective_aec,created_at
                 ) VALUES('voice_sample_legacy','default',1,
                   'voice-profiles/default/voice_sample_legacy.wav.enc',10000,16000,?1,
                   'microphone_test',0,'now')",
                [vec![0_u8; 64]],
            )
            .expect("legacy sample inserts");
        connection
            .pragma_update(None, "user_version", 14)
            .expect("legacy schema version sets");
        let transaction = connection
            .unchecked_transaction()
            .expect("plaintext migration starts");
        migrate_v14_to_v15(&transaction).expect("plaintext migration succeeds");
        migrate_v14_to_v15(&transaction).expect("plaintext migration repeats");
        transaction.commit().expect("plaintext migration commits");
        let runtime = VoiceProfileRuntime::unavailable_for_tests(PathBuf::new());
        runtime
            .reconcile_readiness(&connection)
            .expect("readiness reconciles");
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
        let storage_columns: (i64, i64) = connection
            .query_row(
                "SELECT
                   SUM(CASE WHEN name='embedding' THEN 1 ELSE 0 END),
                   SUM(CASE WHEN name='embedding_ciphertext' THEN 1 ELSE 0 END)
                 FROM pragma_table_info('voice_profile_samples')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("storage columns load");
        assert_eq!(storage_columns, (1, 0));
    }

    #[test]
    fn failed_plaintext_migration_rolls_back_the_legacy_schema_and_profile() {
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_v10_to_v11(&connection).expect("legacy migration succeeds");
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','ready',1,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("legacy profile inserts");
        connection
            .execute(
                "INSERT INTO voice_profile_samples(
                   id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                   embedding_ciphertext,input_device_id,effective_aec,created_at
                 ) VALUES('voice_sample_legacy','default',1,
                   'voice-profiles/default/voice_sample_legacy.wav.enc',10000,16000,?1,
                   'microphone_test',0,'now')",
                [vec![0_u8; 64]],
            )
            .expect("legacy sample inserts");
        connection
            .execute_batch(
                "CREATE TRIGGER block_voice_profile_reset
                 BEFORE DELETE ON voice_profiles
                 BEGIN SELECT RAISE(ABORT, 'blocked for rollback test'); END;",
            )
            .expect("failure trigger creates");

        let transaction = connection
            .unchecked_transaction()
            .expect("plaintext migration starts");
        assert!(migrate_v14_to_v15(&transaction).is_err());
        drop(transaction);

        let sample_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM voice_profile_samples", [], |row| {
                row.get(0)
            })
            .expect("legacy samples remain readable");
        let ciphertext_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('voice_profile_samples')
                 WHERE name='embedding_ciphertext'",
                [],
                |row| row.get(0),
            )
            .expect("legacy schema remains readable");
        assert_eq!(sample_count, 1);
        assert_eq!(ciphertext_column, 1);
    }

    #[test]
    fn database_startup_migrates_v14_voice_storage_and_reopens_idempotently() {
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_v10_to_v11(&connection).expect("legacy migration succeeds");
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','ready',1,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("legacy profile inserts");
        connection
            .execute(
                "INSERT INTO voice_profile_samples(
                   id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                   embedding_ciphertext,input_device_id,effective_aec,created_at
                 ) VALUES('voice_sample_legacy','default',1,
                   'voice-profiles/default/voice_sample_legacy.wav.enc',10000,16000,?1,
                   'microphone_test',0,'now')",
                [vec![0_u8; 64]],
            )
            .expect("legacy sample inserts");
        connection
            .pragma_update(None, "user_version", 14)
            .expect("legacy schema version sets");

        crate::persistence::schema::initialize_database(&connection)
            .expect("database startup migration succeeds");
        crate::persistence::schema::initialize_database(&connection)
            .expect("migrated database reopens");

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version reads");
        let profile_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM voice_profiles", [], |row| row.get(0))
            .expect("profile count reads");
        let embedding_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('voice_profile_samples')
                 WHERE name='embedding'",
                [],
                |row| row.get(0),
            )
            .expect("plaintext schema reads");
        assert_eq!(version, crate::memory::control_plane::MEMORY_SCHEMA_VERSION);
        assert_eq!(profile_count, 0);
        assert_eq!(embedding_column, 1);
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
        migrate_plain_voice_profile_schema(&connection);
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
                       embedding,input_device_id,effective_aec,created_at
                     ) VALUES(?1,'default',?2,?3,10000,16000,?4,'microphone_test',0,'now')",
                    params![
                        format!("sample_{ordinal}"),
                        ordinal as i64,
                        format!("voice-profiles/default/sample_{ordinal}.wav"),
                        encode_embedding(&vec![0.0; 192]),
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
    fn startup_readiness_reconciliation_downgrades_incomplete_profiles() {
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_plain_voice_profile_schema(&connection);
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','ready',1,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        let runtime = VoiceProfileRuntime::unavailable_for_tests(PathBuf::new());
        runtime
            .reconcile_readiness(&connection)
            .expect("readiness reconciles");
        let snapshot = runtime.snapshot(&connection).expect("snapshot loads");
        assert_eq!(snapshot.status, "collecting");
        assert!(!snapshot.filter_enabled);
    }

    #[test]
    fn enrollment_device_must_match_the_active_input_device() {
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_plain_voice_profile_schema(&connection);
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','ready',0,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        for ordinal in 1..=TARGET_SAMPLE_COUNT {
            connection
                .execute(
                    "INSERT INTO voice_profile_samples(
                       id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                       embedding,input_device_id,effective_aec,created_at
                     ) VALUES(?1,'default',?2,?3,10000,16000,?4,'enrollment-mic',0,'now')",
                    params![
                        format!("sample_device_{ordinal}"),
                        ordinal as i64,
                        format!("voice-profiles/default/sample_device_{ordinal}.wav"),
                        encode_embedding(&vec![0.0; 192]),
                    ],
                )
                .expect("sample inserts");
        }
        assert!(enrollment_uses_input_device(&connection, "enrollment-mic")
            .expect("matching device checks"));
        assert!(!enrollment_uses_input_device(&connection, "current-mic")
            .expect("mismatched device checks"));
    }

    #[test]
    fn enabled_filter_requires_available_streaming_verifier() {
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_plain_voice_profile_schema(&connection);
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
            .prepare_streaming_verifier(&connection)
            .err()
            .expect("unavailable verifier rejects");
        assert!(error.starts_with("TARGET_SPEAKER_UNAVAILABLE"));
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
                    "voice-profiles/default/voice_sample_0123456789abcdef.wav",
                )
                .expect("generated path is accepted"),
            PathBuf::from("/private/data/voice-profiles/default/voice_sample_0123456789abcdef.wav")
        );
    }

    #[test]
    fn startup_reconciliation_removes_only_obsolete_owned_voice_files() {
        let directory = tempfile::tempdir().expect("temporary directory creates");
        let profile = directory.path().join("voice-profiles/default");
        fs::create_dir_all(&profile).expect("profile directory creates");
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_plain_voice_profile_schema(&connection);
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','collecting',0,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        connection
            .execute(
                "INSERT INTO voice_profile_samples(
                   id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                   embedding,input_device_id,effective_aec,created_at
                 ) VALUES('voice_sample_current','default',1,
                   'voice-profiles/default/voice_sample_current.wav',10000,16000,?1,
                   'microphone_test',0,'now')",
                [encode_embedding(&vec![0.0; 192])],
            )
            .expect("sample inserts");
        let retained = profile.join("voice_sample_current.wav");
        let orphaned = profile.join("voice_sample_orphaned.wav");
        let legacy = profile.join("voice_sample_legacy.wav.enc");
        let old_temporary =
            profile.join("voice_sample_old.wav.tmp-0123456789abcdef0123456789abcdef");
        let new_temporary = profile.join("voice_sample_new.tmp-0123456789abcdef0123456789abcdef");
        let unrelated = profile.join("notes.wav");
        for path in [
            &retained,
            &orphaned,
            &legacy,
            &old_temporary,
            &new_temporary,
            &unrelated,
        ] {
            fs::write(path, b"RIFF").expect("fixture writes");
        }

        reconcile_voice_profile_storage(&connection, directory.path())
            .expect("storage reconciliation succeeds");

        assert!(retained.exists());
        assert!(unrelated.exists());
        for removed in [orphaned, legacy, old_temporary, new_temporary] {
            assert!(!removed.exists());
        }
    }

    #[test]
    fn storage_reconciliation_preserves_plain_wav_when_metadata_is_inconsistent() {
        let directory = tempfile::tempdir().expect("temporary directory creates");
        let profile = directory.path().join("voice-profiles/default");
        fs::create_dir_all(&profile).expect("profile directory creates");
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_plain_voice_profile_schema(&connection);
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','collecting',0,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        connection
            .execute(
                "INSERT INTO voice_profile_samples(
                   id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                   embedding,input_device_id,effective_aec,created_at
                 ) VALUES('voice_sample_current','default',1,
                   'voice-profiles/default/voice_sample_wrong.wav',10000,16000,?1,
                   'microphone_test',0,'now')",
                [encode_embedding(&vec![0.0; 192])],
            )
            .expect("inconsistent sample inserts");
        let plain = profile.join("voice_sample_orphaned.wav");
        let legacy = profile.join("voice_sample_legacy.wav.enc");
        fs::write(&plain, b"RIFF").expect("plain fixture writes");
        fs::write(&legacy, b"legacy").expect("legacy fixture writes");

        reconcile_voice_profile_storage(&connection, directory.path())
            .expect("storage reconciliation succeeds");

        assert!(plain.exists());
        assert!(!legacy.exists());
    }

    #[test]
    fn failed_sample_file_deletion_retains_metadata_and_retry_converges() {
        let directory = tempfile::tempdir().expect("temporary directory creates");
        let profile = directory.path().join("voice-profiles/default");
        fs::create_dir_all(&profile).expect("profile directory creates");
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_plain_voice_profile_schema(&connection);
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','collecting',0,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        connection
            .execute(
                "INSERT INTO voice_profile_samples(
                   id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                   embedding,input_device_id,effective_aec,created_at
                 ) VALUES('voice_sample_blocked','default',1,
                   'voice-profiles/default/voice_sample_blocked.wav',10000,16000,?1,
                   'microphone_test',0,'now')",
                [encode_embedding(&vec![0.0; 192])],
            )
            .expect("sample inserts");
        let blocked = profile.join("voice_sample_blocked.wav");
        fs::create_dir(&blocked).expect("undeletable-as-file fixture creates");
        let runtime = VoiceProfileRuntime::unavailable_for_tests(directory.path().to_path_buf());

        assert!(runtime
            .delete_sample_from_connection(&connection, "voice_sample_blocked")
            .is_err());
        let retained: i64 = connection
            .query_row("SELECT COUNT(*) FROM voice_profile_samples", [], |row| {
                row.get(0)
            })
            .expect("sample metadata remains readable");
        assert_eq!(retained, 1);

        fs::remove_dir(&blocked).expect("blocking directory removes");
        runtime
            .delete_sample_from_connection(&connection, "voice_sample_blocked")
            .expect("retry succeeds");
        let remaining: i64 = connection
            .query_row("SELECT COUNT(*) FROM voice_profile_samples", [], |row| {
                row.get(0)
            })
            .expect("sample metadata remains readable");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn failed_profile_file_deletion_retains_metadata_and_retry_converges() {
        let directory = tempfile::tempdir().expect("temporary directory creates");
        let profile = directory.path().join("voice-profiles/default");
        fs::create_dir_all(&profile).expect("profile directory creates");
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_plain_voice_profile_schema(&connection);
        connection
            .execute(
                "INSERT INTO voice_profiles(
                   id,status,filter_enabled,threshold,model_sha256,embedding_dimension,created_at,updated_at
                 ) VALUES('default','collecting',0,0.55,?1,192,'now','now')",
                [MODEL_SHA256],
            )
            .expect("profile inserts");
        connection
            .execute(
                "INSERT INTO voice_profile_samples(
                   id,profile_id,ordinal,relative_path,duration_ms,sample_rate,
                   embedding,input_device_id,effective_aec,created_at
                 ) VALUES('voice_sample_blocked','default',1,
                   'voice-profiles/default/voice_sample_blocked.wav',10000,16000,?1,
                   'microphone_test',0,'now')",
                [encode_embedding(&vec![0.0; 192])],
            )
            .expect("sample inserts");
        let blocked = profile.join("voice_sample_blocked.wav");
        fs::create_dir(&blocked).expect("undeletable-as-file fixture creates");
        let runtime = VoiceProfileRuntime::unavailable_for_tests(directory.path().to_path_buf());

        assert!(runtime.delete_profile_from_connection(&connection).is_err());
        let retained: i64 = connection
            .query_row("SELECT COUNT(*) FROM voice_profiles", [], |row| row.get(0))
            .expect("profile metadata remains readable");
        assert_eq!(retained, 1);

        fs::remove_dir(&blocked).expect("blocking directory removes");
        runtime
            .delete_profile_from_connection(&connection)
            .expect("retry succeeds");
        let remaining: i64 = connection
            .query_row("SELECT COUNT(*) FROM voice_profiles", [], |row| row.get(0))
            .expect("profile metadata remains readable");
        assert_eq!(remaining, 0);
    }

    #[cfg(unix)]
    #[test]
    fn plaintext_voice_samples_keep_private_filesystem_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory creates");
        let sample = directory
            .path()
            .join("voice-profiles/default/voice_sample_private.wav");
        write_private_atomic(&sample, b"RIFF").expect("sample writes");

        assert_eq!(
            fs::metadata(sample)
                .expect("sample metadata loads")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(directory.path().join("voice-profiles/default"))
                .expect("directory metadata loads")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
