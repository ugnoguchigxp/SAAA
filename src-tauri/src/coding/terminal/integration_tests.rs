use super::*;
use crate::coding::{contracts::CodingSettings, repository, service};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
#[test]
#[ignore = "set SAAA_TERMINAL_TEST_HELPER to the built desktop binary; isolated DB and fake CLIs only"]
fn dedicated_terminal_roundtrip_uses_production_runner_hooks_mcp_and_writer() {
    use std::os::unix::fs::PermissionsExt;
    assert!(std::env::var_os("SAAA_TERMINAL_TEST_NO_VIEWER").is_some());
    for cli in ["claude", "codex"] {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join(cli);
        std::fs::write(&executable, include_str!("fixture_cli.py")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&workspace)
            .status()
            .unwrap()
            .success());
        let c = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&c).unwrap();
        c.execute("INSERT INTO conversation_messages VALUES('human',?1,'user','Create result.txt; ask me which color','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
        c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('host',?1,'conversation.respond','running','human','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
        let mut state = crate::test_support::app_state(c);
        state.data_directory = root.path().to_owned();
        let settings = CodingSettings {
            enabled: true,
            implementation_method: "terminal".into(),
            terminal_kind: "kitty".into(),
            terminal_cli: cli.into(),
            terminal_executable: executable.to_string_lossy().into_owned(),
            terminal_checks: vec![vec![
                "/usr/bin/grep".into(),
                "--quiet".into(),
                "Blue".into(),
                "result.txt".into(),
            ]],
            ..Default::default()
        };
        state
            .sqlite_writer
            .write(|c| {
                c.execute(
                    "UPDATE coding_settings SET value_json=?1",
                    [serde_json::to_string(&settings).unwrap()],
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
        let registered = service::register(
            &state,
            crate::PRIMARY_CONVERSATION_ID,
            workspace.to_str().unwrap(),
        )
        .unwrap();
        let input = crate::StartTurnInput {
            run_id: "host".into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: "Create result.txt; ask me which color".into(),
            workspace_path: Some(workspace.to_string_lossy().into_owned()),
            retry_input_message_id: None,
            source_id: Some("human".into()),
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let result=service::execute(&state,&input,&crate::runtime::agent_tools::AgentToolCall{id:"start".into(),name:"coding_start".into(),arguments:json!({"workspaceId":registered["workspaceId"],"request":"Create result.txt; ask me which color"}).to_string()}).unwrap();
        let job = result["jobId"].as_str().unwrap();
        let paused = wait(&state, job, "awaiting_user");
        let question = &paused["terminal"]["questions"][0];
        state.sqlite_writer.write(|c|{c.execute("INSERT INTO conversation_messages VALUES('reply',?1,'user','Blue','2')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('reply-host',?1,'conversation.respond','running','reply','2')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();Ok(())}).unwrap();
        let reply = crate::StartTurnInput {
            run_id: "reply-host".into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: "Blue".into(),
            workspace_path: Some(workspace.to_string_lossy().into_owned()),
            retry_input_message_id: None,
            source_id: Some("reply".into()),
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let answer = if cli == "claude" {
            json!({"Which color?":"Blue"})
        } else {
            json!("Blue")
        };
        service::execute(&state,&reply,&crate::runtime::agent_tools::AgentToolCall{id:"answer".into(),name:"coding_answer".into(),arguments:json!({"jobId":job,"questionId":question["questionId"],"expectedRevision":paused["revision"],"answer":answer}).to_string()}).unwrap();
        let completed = wait(&state, job, "completed");
        assert_eq!(completed["result"]["complete"], true);
        assert_eq!(
            std::fs::read_to_string(workspace.join("result.txt")).unwrap(),
            "Blue\n"
        );
        assert_eq!(completed["sessionId"], "fixture-exact-session");
        assert!(completed["terminal"]["questions"]
            .as_array()
            .unwrap()
            .is_empty());
        state
            .sqlite_writer
            .write(|c| {
                c.execute(
                    "UPDATE steward_reports SET speak_requested=0,speech_state='suppressed'",
                    [],
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
        crate::steward::pump::drain(&state).unwrap();
        assert!(!state.steward_wake.emitted().is_empty());
        let delivered = state.steward_wake.emitted().len();
        crate::steward::pump::drain(&state).unwrap();
        assert_eq!(state.steward_wake.emitted().len(), delivered);
        state.sqlite_readers.read(|c|{assert!(c.query_row("SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE role='assistant' AND content LIKE '%確認処理%')",[],|r|r.get::<_,bool>(0)).unwrap());Ok(())}).unwrap();
    }
}
fn wait(state: &crate::AppState, job: &str, expected: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        ingress::drain(state).unwrap();
        let value = state
            .sqlite_readers
            .read(|c| repository::inspect(c, crate::PRIMARY_CONVERSATION_ID, job, 0, 1))
            .unwrap();
        if value["state"] == expected
            && (expected != "awaiting_user" || value["terminal"]["phase"] == "paused")
        {
            return value;
        }
        assert!(Instant::now() < deadline, "expected {expected}: {value}");
        std::thread::sleep(Duration::from_millis(30));
    }
}
