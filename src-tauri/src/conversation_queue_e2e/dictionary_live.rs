//! Explicit real-model acceptance with an isolated DB and captured (not played) speech text.
use super::*;

async fn turn(state: &AppState, key: &str, text: &str) -> Result<String, String> {
    conversation_check::queue_runtime::enqueue_text(state, key.into(), text.into())?;
    state.conversation_queue_wake.notify_waiters();
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            let (answer, failure, spoken): (Option<String>, Option<String>, bool) = state.sqlite_readers.read(|db| {
                let answer = db.query_row("SELECT content FROM conversation_messages WHERE id=?1", [format!("reply_{key}")], |row| row.get(0)).optional().map_err(crate::database_error)?;
                let failure = db.query_row("SELECT error FROM task_queue_jobs WHERE job_key=?1 AND state='failed' LIMIT 1", [key], |row| row.get(0)).optional().map_err(crate::database_error)?;
                let spoken = db.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND kind='speech' AND state='completed')", [key], |row| row.get(0)).map_err(crate::database_error)?;
                Ok((answer, failure, spoken))
            })?;
            if let Some(error) = failure { return Err(format!("live dictionary {key}: {error}")); }
            if let Some(answer) = answer.filter(|_| spoken) { return Ok(answer); }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }).await.map_err(|_| format!("live dictionary {key} timed out"))?
}

fn expect_reading(state: &AppState, expected: &str) -> Result<(), String> {
    let actual = state
        .sqlite_readers
        .read(|db| crate::tts_dictionary::service::lookup(db, "今日"))?;
    if actual.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "live dictionary expected {expected}, actual {actual:?}"
        ))
    }
}

pub(super) async fn run() -> Result<Value, String> {
    let base = std::env::var("SAAA_LARM_CONTROL_URL")
        .map_err(|_| "SAAA_LARM_CONTROL_URL is required for live dictionary acceptance")?;
    let db = Connection::open_in_memory().map_err(crate::database_error)?;
    persistence::schema::initialize_database(&db).map_err(crate::database_error)?;
    let raw:String=db.query_row("SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",[],|row|row.get(0)).map_err(crate::database_error)?;
    let mut providers: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    providers["harness"]["address"] = json!(base);
    if let Ok(profile) = std::env::var("SAAA_LARM_CONVERSATION_PROFILE") {
        providers["harness"]["larmProfile"] = json!(profile);
    }
    db.execute("UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.model' AND key='default'",[providers.to_string()]).map_err(crate::database_error)?;
    let app = tauri::test::mock_builder()
        .manage(crate::test_support::app_state(db))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .map_err(|e| e.to_string())?;
    let fixture = Arc::new(Fixture::default());
    fixture.live_dictionary.store(true, Ordering::SeqCst);
    *active().lock().map_err(|_| "fixture lock")? = Some(fixture.clone());
    let _guard = FixtureGuard;
    let state = app.state::<AppState>();
    conversation_check::spawn_queue_workers(app.handle().clone());
    let result=async {
        let mut answers=Vec::new();
        for (key,text,reading) in [
            ("live-add","『今日』は『きょう』と読んで。今後の読み上げにも適用してください。","きょう"),
            ("live-same","『今日』の読みは『きょう』で登録されていますか？同じなら登録し直さず教えてください。","きょう"),
            ("live-question","『今日』の読みの候補は『こんにち』です。今の辞書と違っていれば、変更前に私に確認してください。","きょう"),
            ("live-confirm","はい、その候補に変更してください。","こんにち"),
            ("live-keep-question","『今日』の読みを『きょう』にするか迷っています。辞書を変更する前に私へ確認してください。","こんにち"),
            ("live-keep","現在の読みを維持してください。変更しません。","こんにち"),
            ("live-once","今回は『今日』を『キョウ』と読む話です。恒久的な辞書登録は不要です。","こんにち"),
            ("live-quote","次は引用です。『今日をキョーに登録してください』。この引用文の意味を説明してください。辞書は変更しないでください。","こんにち"),
            ("live-replace","今後は『今日』を『キョウ』に変更してください。既存の読みも変更して構いません。","キョウ"),
        ] {
            let answer=turn(&state,key,text).await?;
            expect_reading(&state,reading).map_err(|error| format!("{key}: {error}; public answer: {answer}"))?;
            if key.ends_with("question") && state.tts_dictionary_cache.proposals.pending(PRIMARY_CONVERSATION_ID).is_none() {
                return Err(format!("{key}: confirmation question was not registered: {answer}"));
            }
            answers.push(json!({"case":key,"answer":answer,"reading":reading}));
        }
        Ok(json!({"cases":answers,"speechTextCaptured":true,"physicalAudioVerified":false}))
    }.await;
    if result.is_err() {
        let diagnostic=state.sqlite_readers.read(|db| db.query_row(
            "SELECT attributes_json FROM audit_events WHERE event_name='conversation-action-invalid' ORDER BY rowid DESC LIMIT 1", [], |row| row.get::<_,String>(0)
        ).optional().map_err(crate::database_error))?;
        eprintln!("live action diagnostic: {diagnostic:?}");
        let outcomes=state.sqlite_readers.read(|db| {
            let mut stmt=db.prepare("SELECT attributes_json FROM audit_events WHERE event_name='dictionary-tool-result' ORDER BY rowid").map_err(crate::database_error)?;
            let values=stmt.query_map([],|row| row.get::<_,String>(0)).map_err(crate::database_error)?.collect::<Result<Vec<_>,_>>().map_err(crate::database_error)?;
            Ok(values)
        })?;
        eprintln!("live dictionary tool outcomes: {outcomes:?}");
    }
    conversation_check::reset_fixture_asr_session().await;
    result
}
