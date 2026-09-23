use super::*;
#[cfg(unix)]
pub(super) fn run_adapter(live: bool, forget_source: bool) {
    let _lock = ADAPTER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("fixture-pi");
    std::fs::write(&executable, include_str!("../../test_pi.py")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(&workspace)
        .status()
        .unwrap()
        .success());
    let c = database();
    let mut state = crate::test_support::app_state(c);
    state.data_directory = directory.path().to_owned();
    let mut settings = contracts::CodingSettings {
        enabled: true,
        executable: executable.to_string_lossy().into_owned(),
        ..Default::default()
    };
    if live {
        settings.executable = std::env::var("SAAA_PI_EXECUTABLE").unwrap();
        settings.profile = "codex-sdk-v1".into();
        settings.provider = "saaa-codex-sdk".into();
        settings.sdk_extension_path = Some(std::env::var("SAAA_PI_SDK_EXTENSION").unwrap());
    }
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
    let mut input = crate::StartTurnInput {
        run_id: "host".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: "implement".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let request = if live {
        "Create hello.txt containing exactly first\n in the current directory. Use file tools. Then reply DONE."
    } else {
        "implement"
    };
    let mut call = crate::runtime::agent_tools::AgentToolCall {
        id: "first".into(),
        name: "coding_start".into(),
        arguments: json!({"workspaceId":registered["workspaceId"],"request":request}).to_string(),
    };
    let first = service::execute(&state, &input, &call).unwrap();
    assert_eq!(first["state"], "queued");
    let wait = |job: &str| {
        let deadline = Instant::now() + Duration::from_secs(if live { 180 } else { 10 });
        loop {
            let result = state
                .sqlite_readers
                .read(|c| repo::inspect(c, crate::PRIMARY_CONVERSATION_ID, job, 0, 100))
                .unwrap();
            if matches!(
                result["state"].as_str(),
                Some("settled" | "failed" | "interrupted")
            ) {
                return result;
            }
            assert!(Instant::now() < deadline, "{result}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let job = first["jobId"].as_str().unwrap();
    let result = wait(job);
    if live && result["state"] != "settled" {
        let session_path: String = state
            .sqlite_readers
            .read(|c| {
                c.query_row(
                    "SELECT session_path FROM coding_jobs WHERE id=?1",
                    [job],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })
            .unwrap();
        for line in std::fs::read_to_string(session_path).unwrap().lines() {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Some(error) = entry["message"]["errorMessage"].as_str() {
                eprintln!("SDK model error: {error}");
            }
        }
    }
    assert_eq!(result["state"], "settled", "{result}");
    if live {
        assert_eq!(
            std::fs::read_to_string(workspace.join("hello.txt"))
                .unwrap()
                .trim(),
            "first"
        );
    } else {
        assert_eq!(result["result"]["summary"], "fixture result 2");
    }
    call.id = "redelivery".into();
    let duplicate = service::execute(&state, &input, &call).unwrap();
    assert_eq!(duplicate["jobId"], first["jobId"]);
    call.arguments =
        json!({"workspaceId":registered["workspaceId"],"request":"different"}).to_string();
    assert_eq!(
        service::execute(&state, &input, &call).unwrap_err(),
        "idempotency_conflict"
    );
    state.sqlite_writer.write(|c|{c.execute("INSERT INTO conversation_messages VALUES('input2',?1,'user','change it','2')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('host2',?1,'conversation.respond','running','input2','2')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();Ok(())}).unwrap();
    input.run_id = "host2".into();
    call.id = "followup".into();
    call.name = "coding_continue".into();
    call.arguments =
        json!({"jobId":job,"expectedRevision":result["revision"],"request":if live {"Change the file created in our previous turn to contain exactly second\n. Use file tools. Reply DONE."}else{"change it"}})
            .to_string();
    service::execute(&state, &input, &call).unwrap();
    let resumed = wait(job);
    assert_eq!(resumed["state"], "settled", "{resumed}");
    assert_eq!(resumed["sessionId"], result["sessionId"]);
    assert_ne!(resumed["runId"], result["runId"]);
    if live {
        assert_eq!(
            std::fs::read_to_string(workspace.join("hello.txt"))
                .unwrap()
                .trim(),
            "second"
        );
        let session_path: String = state
            .sqlite_readers
            .read(|c| {
                c.query_row(
                    "SELECT session_path FROM coding_jobs WHERE id=?1",
                    [job],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })
            .unwrap();
        let entries: Vec<serde_json::Value> = std::fs::read_to_string(session_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let bindings: Vec<_> = entries
            .iter()
            .filter(|e| e["customType"] == "saaa.codex-sdk.binding")
            .collect();
        assert!(bindings.len() >= 2);
        assert!(bindings
            .iter()
            .all(|e| e["data"]["threadId"] == bindings[0]["data"]["threadId"]));
        assert!(entries
            .iter()
            .any(|e| e["customType"] == "saaa.codex-sdk.observation"));
        println!(
            "Live pi → Codex SDK: start, file edit, session resume, persisted observations passed"
        );
        return;
    }
    assert_eq!(resumed["result"]["summary"], "fixture result 4");
    let mut last = resumed;
    for (source, prompt, expected_state) in [
        ("input3", "wait", "interrupted"),
        ("input4", "model error", "failed"),
    ] {
        let host = format!("host_{source}");
        state.sqlite_writer.write(|c|{c.execute("INSERT INTO conversation_messages VALUES(?1,?2,'user',?3,'3')",params![source,crate::PRIMARY_CONVERSATION_ID,prompt]).unwrap();c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES(?1,?2,'conversation.respond','running',?3,'3')",params![host,crate::PRIMARY_CONVERSATION_ID,source]).unwrap();Ok(())}).unwrap();
        input.run_id = host;
        call.id = source.into();
        call.arguments =
            json!({"jobId":job,"expectedRevision":last["revision"],"request":prompt}).to_string();
        service::execute(&state, &input, &call).unwrap();
        if prompt == "wait" {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let current = state
                    .sqlite_readers
                    .read(|c| repo::inspect(c, crate::PRIMARY_CONVERSATION_ID, job, 0, 1))
                    .unwrap();
                if current["delivery"] == "accepted" {
                    if forget_source {
                        state
                            .sqlite_writer
                            .write(|c| {
                                c.execute(
                                    "DELETE FROM conversation_messages WHERE id=?1",
                                    [source],
                                )
                                .map_err(crate::database_error)?;
                                Ok(())
                            })
                            .unwrap();
                    } else {
                        let receipt = state
                            .sqlite_writer
                            .write(|c| {
                                service::cancel(
                                    c,
                                    crate::PRIMARY_CONVERSATION_ID,
                                    job,
                                    current["revision"].as_u64().unwrap(),
                                    "stop",
                                )
                            })
                            .unwrap();
                        assert_eq!(receipt["state"], "cancel_requested");
                    }
                    break;
                }
                assert!(Instant::now() < deadline, "{current}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        if prompt == "wait" && forget_source {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let (job_state, run_state): (String, String) = state
                    .sqlite_readers
                    .read(|c| {
                        c.query_row(
                            "SELECT j.state,r.state FROM coding_jobs j
                             JOIN coding_runs r ON r.id=j.current_run_id
                             WHERE j.id=?1",
                            [job],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .map_err(crate::database_error)
                    })
                    .unwrap();
                if job_state == "interrupted" {
                    assert_eq!(run_state, "interrupted");
                    break;
                }
                assert!(Instant::now() < deadline, "{job_state}/{run_state}");
                std::thread::sleep(Duration::from_millis(20));
            }
            // A forgotten source invalidates the whole job lineage, so a
            // follow-up run is intentionally not authorized.
            break;
        }
        last = wait(job);
        assert_eq!(last["state"], expected_state, "{last}");
        if prompt == "model error" {
            assert_eq!(last["result"]["modelErrors"], 1);
        }
    }
}
// Explicit local setup command. Not part of normal test runs.
#[test]
#[ignore = "writes coding settings to the explicitly selected app database"]
pub(super) fn configure_local_pi_codex_sdk() {
    use crate::coding::{contracts::CodingSettings, repository};
    let database = std::path::PathBuf::from(std::env::var("SAAA_CODING_SETUP_DATABASE").unwrap());
    assert!(database.is_absolute() && database.is_file());
    let settings = CodingSettings {
        enabled: true,
        executable: std::env::var("SAAA_PI_EXECUTABLE").unwrap(),
        provider: "saaa-codex-sdk".into(),
        profile: "codex-sdk-v1".into(),
        sdk_extension_path: Some(std::env::var("SAAA_PI_SDK_EXTENSION").unwrap()),
        ..Default::default()
    };
    crate::runtime::pi::process::probe(&settings, database.parent().unwrap()).unwrap();
    let writer = crate::persistence::sqlite::SqliteWriter::open(&database).unwrap();
    writer.write(|c| {
        let active: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM coding_runs WHERE state IN ('starting','running','stopping'))", [], |r|r.get(0)).map_err(crate::database_error)?;
        if active { return Err("coding_busy".into()); }
        let previous = serde_json::to_vec_pretty(&repository::settings(c)?).unwrap();
        let backup = database.with_extension(format!("coding-settings-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(backup,previous).map_err(|e| e.to_string())?;
        c.execute("UPDATE coding_settings SET value_json=?1 WHERE id=1",[serde_json::to_string(&settings).unwrap()]).map_err(crate::database_error)?;
        assert_eq!(repository::settings(c)?.profile,"codex-sdk-v1");
        Ok(())
    }).unwrap();
    println!("Saved enabled pi 0.86.1 / Codex SDK / gpt-5.6-luna configuration");
}
