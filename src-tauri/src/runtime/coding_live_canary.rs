use std::sync::Arc;

use rusqlite::Connection;

use crate::{ipc_contract::RuntimeEvent, RunCancellation, StartTurnInput};

#[tokio::test]
#[ignore = "explicit live canary: requires ChatGPT login and network access"]
async fn saaa_luna_persists_and_resumes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut connection = Connection::open_in_memory().expect("database");
    crate::initialize_database(&connection).expect("schema");
    connection
        .execute(
            "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
         VALUES('luna-canary','Luna canary','coding','1','1')",
            [],
        )
        .expect("conversation");
    let mut documents = crate::test_support::default_settings_input();
    let settings = documents
        .iter_mut()
        .find(|d| d.namespace == "providers.agent")
        .unwrap();
    assert_eq!(settings.value_json["model"], "gpt-5.6-luna");
    settings.value_json["enabled"] = true.into();
    crate::persistence::save_settings_documents_to_connection(&mut connection, &documents)
        .expect("settings");
    let state = crate::test_support::app_state(connection);
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let mut thread_id = None;
    for (index, prompt) in [
        "Remember SAAA_LUNA_4827. Reply with exactly that marker. Do not use tools.",
        "Reply with exactly the marker from my previous message. Do not use tools.",
    ]
    .iter()
    .enumerate()
    {
        let input = StartTurnInput {
            run_id: format!("luna-canary-{index}"),
            conversation_id: "luna-canary".into(),
            content: (*prompt).into(),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        crate::runtime::turns::execute_turn(
            &state,
            &input,
            &events,
            Arc::new(RunCancellation::default()),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("SAAA turn failed: {}", error.message));
        state
            .sqlite_readers
            .read(|connection| {
                let id: String = connection
                    .query_row(
                        "SELECT thread_id FROM codex_threads WHERE conversation_id='luna-canary'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)?;
                if let Some(previous) = &thread_id {
                    assert_eq!(previous, &id);
                }
                thread_id = Some(id);
                let content: String = connection.query_row(
                "SELECT content FROM conversation_messages WHERE conversation_id='luna-canary'
                 AND role='assistant' ORDER BY rowid DESC LIMIT 1", [],
                |row| row.get(0)).map_err(crate::database_error)?;
                assert!(content.contains("SAAA_LUNA_4827"));
                Ok(())
            })
            .expect("persisted result");
    }
    assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
}
