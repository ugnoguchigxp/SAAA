use super::*;
// The legacy specialist/TTS event hub no longer exists. The current conversation queue has
// specialist-return and speech/reconnect tests; this boundary verifies that the removed entry
// cannot silently dispatch an untracked provider or synthesize an answer from voice input.
#[tokio::test]
pub(super) async fn removed_voice_conversation_entry_fails_closed_without_answer_or_speech() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "removed-voice-run".into(), conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "find the requested record".into(), workspace_path: None,
        retry_input_message_id: None, source_id: Some("asr-final-1".into()),
        scope_refs: vec![], input_origin: "voice".into(), presentation_mode: "visual-and-spoken".into(),
    };
    let events = tauri::ipc::Channel::<RuntimeEvent>::new(|_| Ok(()));
    let result = execute_turn(&state, &input, &events, Arc::new(RunCancellation::default()), None).await;
    assert!(result.is_err(), "removed executor must not report a completed turn");
    let db = state.sqlite_writer.lock().expect("database lock");
    let answers: i64 = db.query_row("SELECT count(*) FROM conversation_messages WHERE role='assistant'", [], |r| r.get(0)).unwrap();
    assert_eq!(answers, 0);
    let speech: i64 = db.query_row("SELECT count(*) FROM rr_speech WHERE root_id=?1", [&input.run_id], |r| r.get(0)).unwrap();
    assert_eq!(speech, 0);
    let input_count: i64 = db.query_row("SELECT count(*) FROM conversation_messages WHERE content=?1 AND role IN ('user','transcript')", [&input.content], |r| r.get(0)).unwrap();
    assert_eq!(input_count, 1, "voice input remains recorded even if its executor is unavailable");
}
