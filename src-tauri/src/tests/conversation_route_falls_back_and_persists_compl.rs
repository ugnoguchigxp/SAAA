use super::*;

async fn assert_legacy_route_refused(steps: Vec<LlmHttpStep>) {
    let (_, captured, server) = spawn_llm_http_fixture(steps).await;
    let connection = Connection::open_in_memory().unwrap();
    initialize_database(&connection).unwrap();
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "run_retired-route".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "normal conversation".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: vec![],
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let result = execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await;
    assert!(result
        .unwrap_err()
        .message
        .contains("legacy conversation runtime was removed"));
    assert!(captured.lock().unwrap().is_empty());
    let saved: i64 = state
        .sqlite_readers
        .read(|c| {
            c.query_row(
                "SELECT count(*) FROM conversation_messages WHERE role='assistant'",
                [],
                |r| r.get(0),
            )
            .map_err(database_error)
        })
        .unwrap();
    assert_eq!(saved, 0);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
#[tokio::test]
pub(super) async fn retired_conversation_route_refuses_before_starting_a_fallback() {
    assert_legacy_route_refused(vec![LlmHttpStep::Fail]).await;
}
#[tokio::test]
pub(super) async fn retired_stream_cannot_publish_partial_output_or_start_a_fallback() {
    assert_legacy_route_refused(vec![LlmHttpStep::Delta("partial"), LlmHttpStep::Complete]).await;
}
#[test]
pub(super) fn codex_thread_mapping_survives_database_reopen() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("threads.sqlite3");
    let connection = Connection::open(&path).expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('coding-conversation', NULL, 'coding', 'now', 'now')",
            [],
        )
        .expect("conversation inserts");
    let state = app_state(connection);
    persist_codex_thread(
        &state,
        "coding-conversation",
        "thread-persisted",
        "gpt-test",
        directory.path(),
    )
    .expect("thread persists");
    drop(state);

    let reopened = Connection::open(path).expect("database reopens");
    initialize_database(&reopened).expect("database reinitializes");
    let thread_id: String = reopened
        .query_row(
            "SELECT thread_id FROM codex_threads WHERE conversation_id = 'coding-conversation'",
            [],
            |row| row.get(0),
        )
        .expect("thread reloads");
    assert_eq!(thread_id, "thread-persisted");
}
#[test]
pub(super) fn audio_resampling_and_cancellation_are_bounded_and_idempotent() {
    let input = (0..48_000)
        .map(|index| (index as f32 / 48_000.0) * 2.0 - 1.0)
        .collect::<Vec<_>>();
    let output = voice::network_asr::resample_pcm(&input, 48_000, 16_000);
    assert_eq!(output.len(), 16_000);
    assert!(output.iter().all(|sample| (-1.0..=1.0).contains(sample)));
    let cancellation = RunCancellation::default();
    cancellation.cancel();
    cancellation.cancel();
    assert!(cancellation.is_cancelled());
}
#[cfg(target_os = "macos")]
#[test]
pub(super) fn macos_system_tts_runtime_is_available() {
    assert!(Command::new("say")
        .args(["-v", "?"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("system say command starts")
        .success());
}
