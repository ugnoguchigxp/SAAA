use super::*;
use uuid::Uuid;

struct PreparedVoiceEnrollmentSample {
    sample_id: String,
    relative_path: PathBuf,
    absolute_path: PathBuf,
    duration_ms: u64,
    embedding: Zeroizing<Vec<f32>>,
    input_device_id: String,
    effective_aec: bool,
    created_at: String,
    persisted: bool,
}

impl Drop for PreparedVoiceEnrollmentSample {
    fn drop(&mut self) {
        if !self.persisted {
            let _ = fs::remove_file(&self.absolute_path);
        }
    }
}

impl VoiceProfileRuntime {
    pub(crate) fn save_sample(
        &self,
        writer: &SqliteWriter,
        input: SaveVoiceEnrollmentSampleInput,
    ) -> Result<VoiceProfileSnapshot, String> {
        self.with_mutation(|| {
            let prepared = self.prepare_sample(input)?;
            writer.write(|connection| self.save_prepared_sample(connection, prepared))
        })
    }

    fn prepare_sample(
        &self,
        mut input: SaveVoiceEnrollmentSampleInput,
    ) -> Result<PreparedVoiceEnrollmentSample, String> {
        let input_samples = Zeroizing::new(std::mem::take(&mut input.samples));
        validate_enrollment_input(&input, &input_samples)?;
        let extractor = self.extractor.as_ref().ok_or_else(|| {
            format!(
                "Speaker enrollment is unavailable: {}",
                self.runtime_message
            )
        })?;
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
        let wav = Zeroizing::new(encode_pcm16_wav(&canonical, CANONICAL_SAMPLE_RATE));
        let relative_path = PathBuf::from("voice-profiles")
            .join(PROFILE_ID)
            .join(format!("{sample_id}.wav"));
        let absolute_path = self.data_directory.join(&relative_path);
        write_private_atomic(&absolute_path, &wav)?;
        Ok(PreparedVoiceEnrollmentSample {
            sample_id,
            relative_path,
            absolute_path,
            duration_ms,
            embedding,
            input_device_id: input.input_device_id,
            effective_aec: input.effective_aec,
            created_at: now_iso(),
            persisted: false,
        })
    }

    fn save_prepared_sample(
        &self,
        connection: &Connection,
        mut prepared: PreparedVoiceEnrollmentSample,
    ) -> Result<VoiceProfileSnapshot, String> {
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
        let embedding_dimension = prepared.embedding.len();
        if existing >= 2 {
            let stored_references = connection
                .prepare(
                    "SELECT embedding FROM voice_profile_samples
                     WHERE profile_id=?1 ORDER BY ordinal",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map([PROFILE_ID], |row| row.get::<_, Vec<u8>>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(database_error)?;
            let references = stored_references
                .into_iter()
                .map(|stored| decode_embedding(&stored, embedding_dimension).map(Zeroizing::new))
                .collect::<Result<Vec<_>, _>>()?;
            let best_score = references
                .iter()
                .map(|reference| cosine_similarity(reference, &prepared.embedding))
                .fold(f32::NEG_INFINITY, f32::max);
            if !best_score.is_finite() || best_score < ENROLLMENT_CONSISTENCY_THRESHOLD {
                return Err(
                    "This sample does not match the existing enrollment samples. Record only your own voice and retry"
                        .to_string(),
                );
            }
        }
        let encoded_embedding = Zeroizing::new(encode_embedding(&prepared.embedding));
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
                        embedding_dimension as i64,
                        &prepared.created_at
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
                       embedding,input_device_id,effective_aec,created_at
                    ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        &prepared.sample_id,
                        PROFILE_ID,
                        ordinal,
                        prepared.relative_path.to_string_lossy(),
                        prepared.duration_ms as i64,
                        CANONICAL_SAMPLE_RATE,
                        encoded_embedding.as_slice(),
                        &prepared.input_device_id,
                        prepared.effective_aec,
                        &prepared.created_at,
                    ],
                )
                .map_err(database_error)?;
            update_profile_readiness(&transaction)?;
            transaction.commit().map_err(database_error)
        })();
        database_result?;
        prepared.persisted = true;
        self.snapshot(connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared_sample(data_directory: &Path, suffix: usize) -> PreparedVoiceEnrollmentSample {
        let sample_id = format!("voice_sample_prepared_{suffix}");
        let relative_path = PathBuf::from("voice-profiles")
            .join(PROFILE_ID)
            .join(format!("{sample_id}.wav"));
        let absolute_path = data_directory.join(&relative_path);
        fs::create_dir_all(absolute_path.parent().expect("sample parent exists"))
            .expect("sample directory creates");
        fs::write(&absolute_path, b"RIFF-prepared").expect("prepared sample writes");
        PreparedVoiceEnrollmentSample {
            sample_id,
            relative_path,
            absolute_path,
            duration_ms: 10_000,
            embedding: Zeroizing::new(vec![0.1; 192]),
            input_device_id: "microphone_test".to_string(),
            effective_aec: false,
            created_at: "now".to_string(),
            persisted: false,
        }
    }

    #[test]
    fn prepared_sample_file_is_kept_only_after_database_commit() {
        let directory = tempfile::tempdir().expect("temporary directory creates");
        let connection = Connection::open_in_memory().expect("database opens");
        migrate_v10_to_v11(&connection).expect("legacy migration succeeds");
        let transaction = connection
            .unchecked_transaction()
            .expect("plaintext migration starts");
        migrate_v14_to_v15(&transaction).expect("plaintext migration succeeds");
        transaction.commit().expect("plaintext migration commits");
        let runtime = VoiceProfileRuntime::unavailable_for_tests(directory.path().to_path_buf());

        for suffix in 1..=TARGET_SAMPLE_COUNT {
            let prepared = prepared_sample(directory.path(), suffix);
            let path = prepared.absolute_path.clone();
            let snapshot = runtime
                .save_prepared_sample(&connection, prepared)
                .expect("prepared sample commits");
            assert!(path.exists(), "committed sample file is retained");
            assert_eq!(snapshot.sample_count, suffix);
        }

        let rejected = prepared_sample(directory.path(), TARGET_SAMPLE_COUNT + 1);
        let rejected_path = rejected.absolute_path.clone();
        let error = runtime
            .save_prepared_sample(&connection, rejected)
            .expect_err("sample above the maximum is rejected");
        assert!(error.contains("maximum"));
        assert!(
            !rejected_path.exists(),
            "uncommitted prepared sample is removed"
        );
    }

    #[test]
    fn serialized_readers_and_profile_mutations_use_one_lock_order() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let writer = std::sync::Arc::new(SqliteWriter::from_connection(connection));
        let readers = SqliteReaders::serialized(writer.clone());
        let runtime =
            std::sync::Arc::new(VoiceProfileRuntime::unavailable_for_tests(PathBuf::new()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(17));
        let (done_sender, done_receiver) = std::sync::mpsc::channel();

        for index in 0..16 {
            let writer = writer.clone();
            let readers = readers.clone();
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            let done_sender = done_sender.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let result = if index % 2 == 0 {
                    runtime.read_with_snapshot(&readers, |_connection, snapshot| Ok(snapshot))
                } else {
                    runtime.delete_profile(&writer)
                };
                done_sender.send(result).expect("completion sends");
            });
        }
        drop(done_sender);
        barrier.wait();
        for _ in 0..16 {
            done_receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("profile operation does not deadlock")
                .expect("profile operation succeeds");
        }
    }
}
