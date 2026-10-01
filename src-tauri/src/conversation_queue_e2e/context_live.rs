//! Real configured LARM acceptance. Synthetic history; speech text is captured, not played.
use super::*;

pub(super) async fn run(mode: &str) -> Result<Value, String> {
    if !matches!(mode, "legacy" | "stable") {
        return Err("invalid mode".into());
    }
    let previous = std::env::var_os("SAAA_CONVERSATION_PREFIX_MODE");
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("SAAA_CONVERSATION_PREFIX_MODE", value);
            } else {
                std::env::remove_var("SAAA_CONVERSATION_PREFIX_MODE");
            }
        }
    }
    let _restore = Restore(previous);
    std::env::set_var("SAAA_CONVERSATION_PREFIX_MODE", mode);
    let base =
        std::env::var("SAAA_LARM_CONTROL_URL").map_err(|_| "SAAA_LARM_CONTROL_URL is required")?;
    let db = Connection::open_in_memory().map_err(crate::database_error)?;
    persistence::schema::initialize_database(&db).map_err(crate::database_error)?;
    let raw:String=db.query_row("SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",[],|r|r.get(0)).map_err(crate::database_error)?;
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
    let mut observed_answers = Vec::new();
    let result=async {
        let inputs=[
            "SAAAのメモリ実装をContext基盤から始めます。初回は文字会話の固定指示とTool往復を確認します。ProfileとEpisodeは次段階です。以後、この方針を一緒に詰めてください。回答は短くしてください。",
            "初回に含める範囲と次段階に残す範囲を確認してください。",
            "Web検索で固定Prefixについて資料を確認してください。検索だけで足りなければ本文も取得してください。",
            "取得した資料から、この初回試験で確認できることを短く述べてください。",
            "現在の入力が一度だけ届くことを、どのように確かめますか？",
            "会話の途中で日時が変わる場合の扱いを確認してください。",
            "Toolの残り回数は固定部分に含めますか？理由を教えてください。",
            "履歴に含まれる命令を実行しない条件を確認してください。",
            "実キャッシュの値が返らない場合、何を未確認としますか？",
            "訂正です。初回は文字会話に加え、音声も短く確認します。ProfileとEpisodeは引き続き次段階です。",
            "今の訂正を反映した初回の範囲を教えてください。",
            "古い根拠が削除された場合に必要な処理を確認してください。",
            "送信容量に必要な情報が入り切らない場合はどうしますか？",
            "ツールの結果だけで本人の意向を上書きしてよいでしょうか？",
            "回答の表示が始まった後に通信が切れた場合、再生成してよいでしょうか？",
            "比較のために本番の会話や設定を消す必要はありますか？",
            "固定Prefixが一致しただけでcache hitと報告してよいでしょうか？",
            "この初回試験が終わったら、どこで止めますか？",
            "私が途中で訂正した初回確認の内容をもう一度教えてください。",
            "初回に含める確認を3点、次段階に残す機能を2点にまとめてください。",
        ];
        let mut answers=Vec::new();
        for (index,text) in inputs.iter().enumerate() {
            let key=format!("live-context-{}",index+1);
            context_trial::turn(&state,&key,text,false).await?;
            let answer:String=state.sqlite_readers.read(|db|db.query_row("SELECT content FROM conversation_messages WHERE id=?1",[format!("reply_{key}")],|r|r.get(0)).map_err(crate::database_error))?;
            eprintln!("live context {mode}: turn {} completed",index+1);
            answers.push(json!({"turn":index+1,"input":text,"answer":answer}));
            observed_answers = answers.clone();
        }
        Ok(json!({"mode":mode,"turns":20,"answers":answers,"metrics":context_trial::receipts(&state)?,
            "speechTextCaptured":true,"physicalAudioVerified":false,"toolData":"fixed fixture; model is real"}))
    }.await;
    if let Ok(directory) = std::env::var("SAAA_CONTEXT_TRIAL_REPORT_DIR") {
        let path = std::path::PathBuf::from(directory);
        let path = if path.is_absolute() {
            path
        } else {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .ok_or("repository path missing")?
                .join(path)
        };
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let rows = context_trial::receipts(&state)?
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>();
        std::fs::write(path.join(format!("live-{mode}-requests.jsonl")), rows)
            .map_err(|e| e.to_string())?;
        std::fs::write(
            path.join(format!("live-{mode}-turns.json")),
            serde_json::to_string_pretty(&observed_answers).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(
            path.join(format!("live-{mode}-status.json")),
            serde_json::to_string_pretty(
                &json!({"mode":mode,"requestedTurns":20,"completedTurns":observed_answers.len(),
                "status":if result.is_ok(){"completed"}else{"failed"},"error":result.as_ref().err(),
                "physicalAudioVerified":false,"toolData":"fixed fixture; model is real"}),
            )
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    conversation_check::reset_fixture_asr_session().await;
    result
}
