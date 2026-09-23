use super::*;
    fn write_wave(path: &Path, byte_rate: u32, data_bytes: u32) {
        let mut wave = Vec::with_capacity(44 + data_bytes as usize);
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36_u32 + data_bytes).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16_u32.to_le_bytes());
        wave.extend_from_slice(&1_u16.to_le_bytes());
        wave.extend_from_slice(&1_u16.to_le_bytes());
        wave.extend_from_slice(&16_000_u32.to_le_bytes());
        wave.extend_from_slice(&byte_rate.to_le_bytes());
        wave.extend_from_slice(&2_u16.to_le_bytes());
        wave.extend_from_slice(&16_u16.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&data_bytes.to_le_bytes());
        wave.resize(44 + data_bytes as usize, 0);
        fs::write(path, wave).expect("wave fixture writes");
    }

    #[test]
    fn disabling_a_session_cancels_and_removes_it() {
        let runtime = StreamingSpeechRuntime::default();
        let (work, _receiver) = mpsc::channel(1);
        let cancellation = Arc::new(RunCancellation::default());
        let (idle_reset, _idle_resets) = watch::channel(None);
        runtime.sessions.lock().expect("sessions lock").insert(
            "run-disable".to_string(),
            SpeechSession {
                accumulator: SentenceAccumulator::default(),
                work,
                cancellation: cancellation.clone(),
                child: Arc::new(Mutex::new(None)),
                closed: false,
                enabled: true,
                idle_timer: None,
                idle_reset,
                writer: None,
                expression: Default::default(),
                expression_locked: false,
            },
        );

        runtime.set_enabled("run-disable", false);

        assert!(cancellation.is_cancelled());
        assert!(!runtime.is_active());
    }

    #[test]
    fn adaptive_concurrency_is_bounded_and_uses_recent_latency_ratio() {
        assert_eq!(adaptive_render_concurrency(&VecDeque::new()), 2);
        assert_eq!(
            adaptive_render_concurrency(&VecDeque::from([(100, 1_000), (120, 1_000)])),
            1
        );
        assert_eq!(
            adaptive_render_concurrency(&VecDeque::from([
                (1_900, 1_000),
                (2_100, 1_000),
                (2_900, 1_000),
            ])),
            3
        );
        assert_eq!(
            adaptive_render_concurrency(&VecDeque::from([(99_000, 1)])),
            MAX_RENDER_CONCURRENCY
        );
    }

    #[test]
    fn player_and_render_ahead_share_the_three_chunk_bound() {
        assert_eq!(render_slots_used(0, 2, true), MAX_READY_CHUNKS);
        assert_eq!(render_slots_used(2, 1, false), MAX_READY_CHUNKS);
        assert!(render_slots_used(1, 0, true) < MAX_READY_CHUNKS);
    }

    #[test]
    fn wave_duration_uses_byte_rate_and_rejects_invalid_headers() {
        let path = std::env::temp_dir().join(format!(
            "saaa-tts-duration-{}.wav",
            uuid::Uuid::new_v4().simple()
        ));
        write_wave(&path, 32_000, 16_000);
        assert_eq!(wave_duration_ms(&path), Ok(500));
        fs::write(&path, b"not-wave").expect("invalid wave fixture writes");
        assert!(wave_duration_ms(&path).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_rendered_artifact_is_removed_during_finalization() {
        let path = std::env::temp_dir().join(format!(
            "saaa-tts-invalid-artifact-{}.wav",
            uuid::Uuid::new_v4().simple()
        ));
        fs::write(&path, b"not-wave").expect("invalid wave fixture writes");

        assert!(finalize_rendered_chunk(0, Instant::now(), path.clone(), Instant::now()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn cancel_and_shutdown_are_idle_without_sessions() {
        let runtime = StreamingSpeechRuntime::default();
        assert!(!runtime.is_active());
        runtime.cancel("run_missing");
        runtime.shutdown();
        runtime.set_enabled("run_missing", true);
        runtime.schedule_idle("run_missing", 1);
        assert_eq!(
            runtime.append("run_missing", "Hello."),
            Ok(AppendOutcome::default())
        );
        assert_eq!(
            runtime.flush_idle("run_missing", 1),
            Ok(AppendOutcome::default())
        );
        runtime.finish("run_missing", "Hello.").unwrap();
    }

    #[tokio::test]
    async fn queued_utterance_and_final_response_preserve_order_with_backpressure() {
        let runtime = StreamingSpeechRuntime::default();
        let (work, mut receiver) = mpsc::channel(1);
        let cancellation = Arc::new(RunCancellation::default());
        let (idle_reset, _idle_resets) = watch::channel(None);
        runtime.sessions.lock().expect("sessions lock").insert(
            "run-response".to_string(),
            SpeechSession {
                accumulator: SentenceAccumulator::default(),
                work,
                cancellation,
                child: Arc::new(Mutex::new(None)),
                closed: false,
                enabled: true,
                idle_timer: None,
                idle_reset,
                writer: None,
                expression: Default::default(),
                expression_locked: false,
            },
        );
        runtime
            .queue_utterance("run-response", "確認しています。")
            .unwrap();
        runtime.finish("run-response", "最終回答です。").unwrap();

        let Some(SpeechWork::Chunk { text, .. }) = receiver.recv().await else {
            panic!("acknowledgement chunk is queued first");
        };
        assert_eq!(text, "確認しています。");
        let Some(SpeechWork::Chunk { text, .. }) = receiver.recv().await else {
            panic!("final response chunk follows the acknowledgement");
        };
        assert_eq!(text, "最終回答です。");
        assert!(matches!(receiver.recv().await, Some(SpeechWork::Finish)));
    }

    #[tokio::test]
    async fn final_message_is_correlated_with_speech_only_once() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        crate::begin_simple_runtime_run(
            &state,
            "run-correlated-speech",
            crate::PRIMARY_CONVERSATION_ID,
            "voice.speak",
            "tts-provider",
        )
        .expect("runtime starts");
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('message-correlated-speech',?1,'assistant','Hello.','1')",
                        [crate::PRIMARY_CONVERSATION_ID],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("message inserts");
        let runtime = StreamingSpeechRuntime::default();
        let (work, mut receiver) = mpsc::channel(4);
        let (idle_reset, _idle_resets) = watch::channel(None);
        runtime.sessions.lock().expect("sessions lock").insert(
            "run-correlated-speech".to_string(),
            SpeechSession {
                accumulator: SentenceAccumulator::default(),
                work,
                cancellation: Arc::new(RunCancellation::default()),
                child: Arc::new(Mutex::new(None)),
                closed: false,
                enabled: true,
                idle_timer: None,
                idle_reset,
                writer: Some(state.sqlite_writer.clone()),
                expression: Default::default(),
                expression_locked: false,
            },
        );

        runtime
            .finish_message(
                "run-correlated-speech",
                "message-correlated-speech",
                "Hello.",
            )
            .expect("first delivery queues");
        runtime
            .finish_message(
                "run-correlated-speech",
                "message-correlated-speech",
                "Hello.",
            )
            .expect("duplicate delivery is ignored");
        while !matches!(receiver.recv().await, Some(SpeechWork::Finish) | None) {}

        let count: i64 = state
            .sqlite_writer
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT COUNT(*) FROM speech_deliveries WHERE runtime_run_id='run-correlated-speech' AND message_id='message-correlated-speech'",
                [],
                |row| row.get(0),
            )
            .expect("delivery count reads");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn begin_creates_a_disabled_session_that_cancel_removes() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let runtime = StreamingSpeechRuntime::default();
        let channel = tauri::ipc::Channel::new(|_| Ok(()));
        runtime
            .begin(&state, "run_speech", false, channel, None)
            .await
            .expect("speech session begins");
        assert!(runtime.is_active());
        assert_eq!(
            runtime.append("run_speech", "Hello."),
            Ok(AppendOutcome::default())
        );
        runtime.schedule_idle("run_speech", 0);
        runtime.flush_idle("run_speech", 0).unwrap();
        runtime.finish("run_speech", "").unwrap();
        runtime.set_enabled("run_speech", false);
        assert!(!runtime.is_active());
    }
