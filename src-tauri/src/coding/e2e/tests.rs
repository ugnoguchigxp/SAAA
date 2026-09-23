//! Explicit live E2E through SAAA's normal conversation runtime and persisted DB.
use crate::{
    ipc_contract::RuntimeEvent,
    persistence::{SqliteReaders, SqliteWriter},
    RunCancellation, StartTurnInput,
};
use rusqlite::Connection;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

mod setup;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "explicit live conversation request; writes to selected app DB and BBS workspace"]
async fn saaa_conversation_builds_bbs_via_pi() {
    let database = PathBuf::from(std::env::var("SAAA_BBS_DATABASE").expect("explicit app DB"));
    let workspace =
        PathBuf::from(std::env::var("SAAA_BBS_WORKSPACE").expect("explicit BBS workspace"))
            .canonicalize()
            .unwrap();
    let report = PathBuf::from(std::env::var("SAAA_BBS_REPORT").expect("explicit evidence file"));
    assert!(database.is_absolute() && database.is_file());
    assert!(workspace.join(".git").is_dir());
    let dummy = Connection::open_in_memory().unwrap();
    crate::initialize_database(&dummy).unwrap();
    let mut state = crate::test_support::app_state(dummy);
    state.sqlite_writer =
        Arc::new(SqliteWriter::open(&database).expect("SAAA must be closed; acquire DB ownership"));
    state.sqlite_readers = SqliteReaders::open(&database).unwrap();
    state.data_directory = database.parent().unwrap().to_owned();
    setup::seed(&state).expect("E2E settings seed");
    setup::configure_local_harness(&state).expect("E2E harness route");
    let before = state
        .sqlite_readers
        .read(crate::persistence::list_settings_documents)
        .unwrap();
    let settings = state
        .sqlite_readers
        .read(crate::coding::repository::settings)
        .unwrap();
    assert!(settings.enabled && settings.profile == "codex-sdk-v1");
    crate::coding::service::register(
        &state,
        crate::PRIMARY_CONVERSATION_ID,
        workspace.to_str().unwrap(),
    )
    .unwrap();
    let run_id = crate::new_id("bbs_e2e");
    let request=std::env::var("SAAA_BBS_REQUEST").unwrap_or_else(|_|format!(
        "選択済みの実装先 {} に、piを使って簡易BBSを実装してください。実装開始を明示的に依頼します。投稿者名と本文を入力して投稿でき、投稿一覧が新しい順で表示される日本語画面にしてください。BunのHTTPサーバーとSQLite等で投稿を永続化し、外部依存は最小限にしてください。localhostで起動できること、空投稿の拒否、HTMLを実行せず安全に本文表示すること、スマホでも使える簡潔な画面を希望します。起動手順READMEと投稿・一覧・永続化の動作テストも作成・実行してください。実装はcoding_startからpiへ依頼し、会話だけで完了したと言わないでください。",workspace.display()));
    let input = StartTurnInput {
        run_id: run_id.clone(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: request,
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    println!("SAAA conversation run: {run_id}");
    let cancellation = Arc::new(RunCancellation::default());
    let turn =
        crate::runtime::turns::execute_turn(&state, &input, &events, cancellation.clone(), None);
    let outcome = tokio::time::timeout(Duration::from_secs(280), turn).await;
    if outcome.is_err() {
        cancellation.cancel();
    }
    let outcome = outcome.expect("conversation deadline");
    if let Err(error) = &outcome {
        eprintln!("Conversation failure: {}", error.message);
    }
    let job = state.sqlite_readers.read(|c| {
        c.query_row(
            "SELECT job_id FROM coding_runs WHERE host_run_id=?1 ORDER BY rowid DESC LIMIT 1",
            [&run_id],
            |r| r.get::<_, String>(0),
        )
        .map_err(crate::database_error)
    });
    let job = job.expect("real conversation must issue coding_start; no direct tool injection");
    println!("Accepted coding job: {job}");
    let deadline = Instant::now() + Duration::from_secs(900);
    let result = loop {
        let result = state
            .sqlite_readers
            .read(|c| {
                crate::coding::repository::inspect(c, crate::PRIMARY_CONVERSATION_ID, &job, 0, 100)
            })
            .unwrap();
        if !matches!(
            result["state"].as_str(),
            Some("queued" | "running" | "cancel_requested")
        ) {
            break result;
        }
        if Instant::now() > deadline {
            crate::coding::commands::shutdown(&state);
            panic!("coding deadline exceeded");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    let (provider,reply)=state.sqlite_readers.read(|c|Ok((c.query_row("SELECT provider_id FROM runtime_runs WHERE id=?1",[&run_id],|r|r.get::<_,Option<String>>(0)).map_err(crate::database_error)?,c.query_row("SELECT content FROM conversation_messages WHERE conversation_id=?1 AND role='assistant' ORDER BY rowid DESC LIMIT 1",[crate::PRIMARY_CONVERSATION_ID],|r|r.get::<_,String>(0)).map_err(crate::database_error)?))).unwrap();
    let after = state
        .sqlite_readers
        .read(crate::persistence::list_settings_documents)
        .unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    let evidence = json!({"conversationRunId":run_id,"providerId":provider,"assistantReceipt":reply,"coding":result,"entryPoint":"runtime::turns::execute_turn / conversation.respond","uiDriven":false,"settingsUnchanged":true});
    std::fs::write(&report, serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
    println!(
        "E2E result: {} / evidence {}",
        result["state"],
        report.display()
    );
    assert_eq!(result["state"], "settled", "{result}");
    outcome.unwrap_or_else(|error| panic!("conversation failed after dispatch: {}", error.message));
}
