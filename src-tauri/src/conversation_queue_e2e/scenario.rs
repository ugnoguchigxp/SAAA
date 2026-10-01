//! Queue acceptance scenario and persistence/speech assertions.
use super::*;

pub(super) async fn run_with_server(base: &str, fixture: &Arc<Fixture>) -> Result<Value, String> {
    let connection = Connection::open_in_memory().map_err(crate::database_error)?;
    persistence::schema::initialize_database(&connection).map_err(crate::database_error)?;
    let mut providers: Value = connection.query_row(
        "SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",
        [], |row| row.get::<_, String>(0),
    ).map_err(crate::database_error).and_then(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))?;
    providers["harness"]["address"] = json!(base);
    providers["harness"]["ttsVoice"] = json!("fixture-voice");
    connection.execute("UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.model' AND key='default'",
        [providers.to_string()]).map_err(crate::database_error)?;
    let mut routing: Value = connection.query_row(
        "SELECT value_json FROM settings_documents WHERE namespace='routing.tasks' AND key='default'",
        [], |row| row.get::<_, String>(0),
    ).map_err(crate::database_error).and_then(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))?;
    routing["voiceTranscribe"]["source"] = json!("harness");
    routing["voiceTranscribe"]["providerId"] = Value::Null;
    routing["voiceSpeak"]["source"] = json!("harness");
    routing["voiceSpeak"]["providerId"] = Value::Null;
    connection.execute("UPDATE settings_documents SET value_json=?1 WHERE namespace='routing.tasks' AND key='default'",
        [routing.to_string()]).map_err(crate::database_error)?;
    let state = crate::test_support::app_state(connection);
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .map_err(|error| error.to_string())?;
    let state = app.state::<AppState>();
    state.sqlite_writer.write(|connection| {
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES('fixture_stale_reply',?1,'assistant','古い天気は雨です','1')",
                [PRIMARY_CONVERSATION_ID],
            )
            .map_err(crate::database_error)?;
        Ok(())
    })?;
    let input_id = "queue-e2e-main";
    fixture.asr_no_speech.store(true, Ordering::SeqCst);
    let non_speech =
        conversation_check::transcribe_fixture_audio(&state, "queue-e2e-no-speech").await;
    fixture.asr_no_speech.store(false, Ordering::SeqCst);
    if !non_speech.is_err_and(|error| error.starts_with("ASR_NO_SPEECH:")) {
        return Err("non-speech ASR text was accepted as a user utterance".into());
    }
    let transcript = conversation_check::transcribe_fixture_audio(&state, input_id).await?;
    if transcript != "今日の事実を調べて" {
        return Err(format!("ASR transcript mismatch: {transcript}"));
    }
    let receipt = conversation_check::queue_runtime::enqueue_text(
        &state,
        input_id.into(),
        transcript.clone(),
    )?;
    let duplicate = conversation_check::queue_runtime::enqueue_text(
        &state,
        input_id.into(),
        transcript.clone(),
    )?;
    if receipt.job_id != duplicate.job_id {
        return Err("ASR delivery was not idempotent".into());
    }
    conversation_check::spawn_queue_workers(app.handle().clone());
    state.conversation_queue_wake.notify_waiters();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let done = state.sqlite_readers.read(|connection| {
            let state: Option<String> = connection.query_row(
                "SELECT state FROM task_queue_jobs WHERE scope=?1 AND kind='speech' AND job_key=?2",
                rusqlite::params![PRIMARY_CONVERSATION_ID,input_id], |row| row.get(0),
            ).optional().map_err(crate::database_error)?;
            Ok(state)
        })?;
        if fixture.invalid_reply.load(Ordering::SeqCst) {
            let rejected: bool = state.sqlite_readers.read(|connection| {
                connection.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND kind='user_input' AND state='failed')",
                    [input_id], |row| row.get(0)).map_err(crate::database_error)
            })?;
            if rejected {
                let saved: bool = state.sqlite_readers.read(|connection| {
                    connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1)",
                            [format!("reply_{input_id}")],
                            |row| row.get(0),
                        )
                        .map_err(crate::database_error)
                })?;
                let pending_progress: i64 = state.sqlite_readers.read(|connection| {
                    connection.query_row(
                        "SELECT count(*) FROM task_queue_jobs WHERE job_key=?1 AND kind='progress_speech' AND state IN ('queued','running')",
                        [input_id], |row| row.get(0),
                    ).map_err(crate::database_error)
                })?;
                let spoken = fixture.spoken.lock().map_err(|_| "spoken lock")?.clone();
                if saved
                    || done.is_some()
                    || pending_progress != 0
                    || spoken.iter().any(|text| text != "只今お調べします。")
                {
                    return Err(format!("uncommitted answer leaked to speech: {spoken:?}"));
                }
                return Ok(json!({"rejectedWithoutSpeech":true}));
            }
        }
        if done.as_deref() == Some("completed") {
            break;
        }
        if fixture.authentication_failure.load(Ordering::SeqCst) {
            let failed: bool = state.sqlite_readers.read(|connection| {
                connection.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND kind='user_input' AND state='failed')",
                    [input_id], |row| row.get(0)).map_err(crate::database_error)
            })?;
            if failed {
                break;
            }
        }
        if done.as_deref() == Some("failed") || tokio::time::Instant::now() > deadline {
            let jobs = state
                .sqlite_readers
                .read(|connection| task_queue::snapshot(connection, PRIMARY_CONVERSATION_ID))?;
            let details = jobs
                .iter()
                .map(|job| format!("{}:{}:{:?}", job.kind, job.state, job.error))
                .collect::<Vec<_>>();
            return Err(format!(
                "queue did not complete: {done:?}; jobs: {details:?}"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let (jobs, answer) = state.sqlite_readers.read(|connection| {
        let jobs = task_queue::snapshot(connection, PRIMARY_CONVERSATION_ID)?;
        let answer: Option<String> = connection
            .query_row(
                "SELECT content FROM conversation_messages WHERE id=?1",
                [format!("reply_{input_id}")],
                |row| row.get(0),
            )
            .optional()
            .map_err(crate::database_error)?;
        Ok((jobs, answer))
    })?;
    let states = jobs
        .iter()
        .filter(|job| job.key == input_id)
        .map(|job| (job.kind.clone(), job.state.clone()))
        .collect::<Vec<_>>();
    if states
        .iter()
        .any(|(kind, _)| kind == "ornith_task" || kind == "ornith_result")
    {
        return Err(format!(
            "new input created a legacy two-model job: {states:?}"
        ));
    }
    if fixture.invalid_reply.load(Ordering::SeqCst) {
        let spoken = fixture.spoken.lock().map_err(|_| "spoken lock")?;
        if answer
            .as_deref()
            .is_some_and(|answer| answer.contains("<think>"))
            || spoken.iter().any(|s| s.contains("<think>"))
            || !answer
                .as_deref()
                .is_some_and(|answer| answer.contains("結果を整理する段階で失敗"))
        {
            return Err(format!("invalid model content escaped into a saved or spoken answer: {answer:?}; states={states:?}"));
        }
        return Ok(json!({"rejectedWithoutSpeech":true}));
    }
    if fixture.authentication_failure.load(Ordering::SeqCst) {
        fixture
            .authentication_failure
            .store(false, Ordering::SeqCst);
        let next_key = "auth-recovery-greeting";
        conversation_check::queue_runtime::enqueue_text(
            &state,
            next_key.into(),
            "こんにちは".into(),
        )?;
        state.conversation_queue_wake.notify_waiters();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let ready: bool = state.sqlite_readers.read(|c| {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1)",
                    [format!("reply_{next_key}")],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })?;
            if ready {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                return Err("next input after authentication rejection did not recover".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let llm_calls = *fixture.llm_calls.lock().map_err(|_| "LLM count lock")?;
        let creates = fixture
            .calls
            .lock()
            .map_err(|_| "calls lock")?
            .iter()
            .filter(|call| call.as_str() == "POST /v1/agent-connections")
            .count();
        let spoken = fixture.spoken.lock().map_err(|_| "spoken lock")?.clone();
        if llm_calls < 2
            || creates < 2
            || !states
                .iter()
                .any(|(kind, state)| kind == "user_input" && state == "failed")
            || answer.is_some()
            || spoken
                .iter()
                .any(|speech| speech.contains("Provider authentication failed"))
        {
            return Err(format!("authentication recovery failed: calls={llm_calls}, creates={creates}, states={states:?}"));
        }
        return Ok(json!({"reconnected":true,"llmCalls":llm_calls,"answer":answer}));
    }
    let answer = answer.ok_or("Ornith did not save an answer")?;
    for kind in ["user_input", "speech"] {
        if !states
            .iter()
            .any(|(name, state)| name == kind && state == "completed")
        {
            return Err(format!("{kind} did not complete: {states:?}"));
        }
    }
    let expected_answer = if fixture.fail_after_search.load(Ordering::SeqCst) {
        assert!(answer.contains("結果を整理する段階で失敗"));
        answer.as_str()
    } else {
        "資料では確認済みの事実は42です。"
    };
    let expected_display = if fixture.fail_after_search.load(Ordering::SeqCst) {
        expected_answer.to_string()
    } else {
        format!("{expected_answer}\n\n<!-- saaa:source-links -->\n[出典1: example.invalid](https://example.invalid/report)\n")
    };
    if answer != expected_display {
        return Err(format!("Ornith answer mismatch: {answer}"));
    }
    let spoken = fixture
        .spoken
        .lock()
        .map_err(|_| "spoken lock unavailable")?
        .clone();
    let logged_progress = state.sqlite_readers.read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT m.content FROM conversation_messages m JOIN task_queue_jobs j
             ON m.id='progress_' || j.id
             WHERE j.scope=?1 AND j.job_key=?2 AND m.role='assistant'
             ORDER BY j.generation",
            )
            .map_err(crate::database_error)?;
        let rows = statement
            .query_map(
                rusqlite::params![PRIMARY_CONVERSATION_ID, input_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(crate::database_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(crate::database_error)
    })?;
    let progress_count = logged_progress.len();
    let expected_progress = state.sqlite_readers.read(|connection| {
        logged_progress
            .iter()
            .map(|text| crate::tts_dictionary::apply_saved(connection, text))
            .collect::<Result<Vec<_>, _>>()
    })?;
    if expected_progress
        != spoken
            .iter()
            .take(progress_count)
            .cloned()
            .collect::<Vec<_>>()
    {
        return Err(format!(
            "spoken progress missing from conversation log: {expected_progress:?}"
        ));
    }
    let expected_speech = state
        .sqlite_readers
        .read(|connection| crate::tts_dictionary::apply_saved(connection, expected_answer))?;
    if (progress_count > 0 && spoken.first() != expected_progress.first())
        || spoken.get(progress_count..).unwrap_or_default().join("") != expected_speech
    {
        return Err(format!(
            "TTS did not receive the final Ornith answer: {spoken:?}"
        ));
    }
    if spoken
        .iter()
        .any(|line| line == "もうすこしおまちください。")
    {
        return Err(format!("retired waiting speech was spoken: {spoken:?}"));
    }
    let pending_progress: i64 = state.sqlite_readers.read(|connection| {
        connection.query_row(
            "SELECT count(*) FROM task_queue_jobs WHERE scope=?1 AND job_key=?2 AND kind='progress_speech' AND state IN ('queued','running')",
            rusqlite::params![PRIMARY_CONVERSATION_ID,input_id], |row| row.get(0),
        ).map_err(crate::database_error)
    })?;
    if pending_progress != 0 {
        return Err("waiting speech was not cancelled after the answer".into());
    }
    let final_spoken = spoken.get(progress_count..).unwrap_or_default().to_vec();
    let searches = fixture
        .searches
        .lock()
        .map_err(|_| "search lock unavailable")?
        .clone();
    if searches != ["fixture fact"] {
        return Err(format!("Ornith tool handoff mismatch: {searches:?}"));
    }
    let calls = fixture
        .calls
        .lock()
        .map_err(|_| "calls lock unavailable")?
        .clone();
    let order = ["/asr/v1/audio/transcriptions", "/llm/v1/chat/completions"];
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.contains("/backchannel/v1/chat/completions"))
            .count(),
        0,
        "conversation must not call the Qwen backchannel"
    );
    let mut cursor = 0;
    for path in order {
        let index = calls
            .iter()
            .enumerate()
            .skip(cursor)
            .find(|(_, call)| call.contains(path))
            .map(|(index, _)| index)
            .ok_or_else(|| format!("Missing provider call {path}: {calls:?}"))?;
        cursor = index + 1;
    }
    if fixture.fail_after_search.load(Ordering::SeqCst) {
        if *fixture
            .llm_calls
            .lock()
            .map_err(|_| "LLM count lock unavailable")?
            != 2
        {
            return Err("Ornith repeated a failed tool follow-up".into());
        }
        let failure_kind: Option<String> = state.sqlite_readers.read(|connection| {
            connection
                .query_row(
                    "SELECT json_extract(attributes_json,'$.kind') FROM audit_events
                 WHERE event_name='conversation-provider-failure' ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::database_error)
        })?;
        return Ok(
            json!({"answer":answer,"spoken":final_spoken,"progressSpoken":logged_progress.first(),"waitingSpoken":null,"searches":searches,
            "jobStates":states,"providerFailureKind":failure_kind}),
        );
    }
    let llm_before = *fixture
        .llm_calls
        .lock()
        .map_err(|_| "LLM count lock unavailable")?;
    let greeting_id = "queue-e2e-greeting";
    conversation_check::queue_runtime::enqueue_text(
        &state,
        greeting_id.into(),
        "こんにちは".into(),
    )?;
    state.conversation_queue_wake.notify_waiters();
    let greeting_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let greeting_state = state.sqlite_readers.read(|connection| {
            connection.query_row(
                "SELECT state FROM task_queue_jobs WHERE scope=?1 AND kind='speech' AND job_key=?2",
                rusqlite::params![PRIMARY_CONVERSATION_ID,greeting_id], |row| row.get::<_,String>(0),
            ).optional().map_err(crate::database_error)
        })?;
        if greeting_state.as_deref() == Some("completed") {
            break;
        }
        if greeting_state.as_deref() == Some("failed")
            || tokio::time::Instant::now() > greeting_deadline
        {
            return Err(format!(
                "quick greeting did not complete: {greeting_state:?}"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let greeting_jobs = state
        .sqlite_readers
        .read(|connection| task_queue::snapshot(connection, PRIMARY_CONVERSATION_ID))?;
    if !greeting_jobs
        .iter()
        .any(|job| job.key == greeting_id && job.kind == "user_input" && job.state == "completed")
    {
        return Err("greeting did not reach Ornith".into());
    }
    if *fixture
        .llm_calls
        .lock()
        .map_err(|_| "LLM count lock unavailable")?
        <= llm_before
    {
        return Err("greeting did not call Ornith".into());
    }
    let spoken_after = fixture
        .spoken
        .lock()
        .map_err(|_| "spoken lock unavailable")?
        .clone();
    if spoken_after.last().map(String::as_str) != Some("こんにちは。") {
        return Err(format!("quick greeting speech mismatch: {spoken_after:?}"));
    }
    cancellation::verify(&state, fixture).await?;
    dictionary::verify(&state, fixture).await?;
    let context_report = if fixture.context_trial.load(Ordering::SeqCst) {
        Some(context_trial::verify(&state, fixture).await?)
    } else {
        None
    };
    Ok(
        json!({"contextTrial":context_report,"cancellationVerified":true,"transcript":transcript,"answer":answer,"spoken":final_spoken,"progressSpoken":logged_progress.first(),"waitingSpoken":null,"searches":searches,
        "jobStates":states,"providerCalls":calls,"quickGreeting":spoken_after.last(),
        "speechBeforeDone":fixture.speech_before_done.load(Ordering::SeqCst)}),
    )
}
