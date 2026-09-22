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
