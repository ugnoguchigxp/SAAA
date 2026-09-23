use rusqlite::params;
use super::*;
pub(crate) fn migrate_plain_voice_profile_schema(connection: &Connection) {
        migrate_v10_to_v11(connection).expect("legacy migration succeeds");
        let transaction = connection
            .unchecked_transaction()
            .expect("plaintext migration starts");
        migrate_v14_to_v15(&transaction).expect("plaintext migration succeeds");
        transaction.commit().expect("plaintext migration commits");
    }
#[test]
    pub(super) fn embedding_codec_round_trips_the_expected_dimension() {
        let embedding = [0.25_f32, -0.5, 1.0];
        let encoded = encode_embedding(&embedding);
        let decoded = decode_embedding(&encoded, embedding.len()).expect("embedding decodes");
        assert_eq!(decoded.as_slice(), embedding);
        assert!(decode_embedding(&encoded, embedding.len() + 1).is_err());
    }
#[test]
    pub(super) fn resampling_and_wav_encoding_produce_canonical_audio() {
        let input = vec![0.25_f32; 48_000];
        let resampled = resample_mono(&input, 48_000, 16_000).expect("resamples");
        assert_eq!(resampled.len(), 16_000);
        let wav = encode_pcm16_wav(&resampled, 16_000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 32_000);
    }
#[test]
    pub(super) fn voice_profile_migration_is_idempotent_and_defaults_to_fail_safe() {
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
    pub(super) fn failed_plaintext_migration_rolls_back_the_legacy_schema_and_profile() {
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
    pub(super) fn database_startup_migrates_v14_voice_storage_and_reopens_idempotently() {
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
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
        assert_eq!(profile_count, 0);
        assert_eq!(embedding_column, 1);
    }
#[test]
    pub(super) fn enrollment_requires_five_samples_of_at_least_ten_seconds() {
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
    pub(super) fn quality_check_accepts_continuous_speech_without_requiring_prompt_completion() {
        let continuous = vec![0.05_f32; CANONICAL_SAMPLE_RATE as usize * 10];
        validate_sample_quality(&continuous).expect("continuous speech is accepted");

        let mut sparse = vec![0.0_f32; CANONICAL_SAMPLE_RATE as usize * 10];
        sparse[..CANONICAL_SAMPLE_RATE as usize * 2].fill(0.05);
        let error = validate_sample_quality(&sparse).expect_err("sparse speech rejects");
        assert!(error.contains("全文を読み切る必要はない"));
        assert!(error.contains("自動停止するまで"));
    }
#[test]
    pub(super) fn startup_readiness_reconciliation_downgrades_incomplete_profiles() {
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
    pub(super) fn enrollment_device_must_match_the_active_input_device() {
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
        assert!(enrollment_uses_input_device(&connection, "default")
            .expect("system default accepts one consistent effective device"));
        assert!(!enrollment_uses_input_device(&connection, "current-mic")
            .expect("mismatched device checks"));
        connection
            .execute(
                "UPDATE voice_profile_samples SET input_device_id='other-mic'
                 WHERE profile_id='default' AND ordinal=?1",
                [TARGET_SAMPLE_COUNT as i64],
            )
            .expect("sample device changes");
        assert!(!enrollment_uses_input_device(&connection, "default")
            .expect("system default rejects mixed enrollment devices"));
    }
#[test]
    pub(super) fn enabled_filter_requires_available_streaming_verifier() {
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
    pub(super) fn cosine_similarity_handles_matching_and_empty_vectors() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 0.0001);
        assert_eq!(cosine_similarity(&[], &[]), f32::NEG_INFINITY);
    }
#[test]
    pub(super) fn stored_sample_paths_cannot_escape_the_voice_profile_directory() {
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
    pub(super) fn startup_reconciliation_removes_only_obsolete_owned_voice_files() {
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
    pub(super) fn storage_reconciliation_preserves_plain_wav_when_metadata_is_inconsistent() {
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
    pub(super) fn failed_sample_file_deletion_retains_metadata_and_retry_converges() {
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
