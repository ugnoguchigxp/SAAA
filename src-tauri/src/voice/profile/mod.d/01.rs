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
    if input_device_id == "default" {
        let (total, distinct_devices): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*),COUNT(DISTINCT input_device_id)
                 FROM voice_profile_samples WHERE profile_id=?1",
                [PROFILE_ID],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(database_error)?;
        return Ok(total as usize >= MIN_READY_SAMPLES && distinct_devices == 1);
    }
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
