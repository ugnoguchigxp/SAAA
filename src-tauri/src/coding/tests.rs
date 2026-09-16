use super::{contracts, repository as repo, service};
use rusqlite::{params, Connection};
use serde_json::json;
fn database() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute(
        "INSERT INTO conversation_messages VALUES('input',?1,'user','please implement','1')",
        [crate::PRIMARY_CONVERSATION_ID],
    )
    .unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('host',?1,'conversation.respond','running','input','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    c
}
fn job(c: &Connection) {
    c.execute("INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id) VALUES('job',?1,'input','workspace','/tmp','{}',1,'/tmp/session','queued','run')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    c.execute("INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES('run','job','input','host','do work','digest','prepared','starting','1')",[]).unwrap();
}
#[test]
fn coding_arguments_require_exact_schema_and_unicode_character_bounds() {
    assert!(contracts::validate(
        "coding_start",
        &json!({"workspaceId":"w","request":"日".repeat(32000)}).to_string()
    )
    .is_ok());
    for value in [
        json!({"workspaceId":"w","request":"x","argv":[]}),
        json!({"workspaceId":"w","request":"x".repeat(32001)}),
        json!({"workspaceId":"w","request":" "}),
    ] {
        assert!(contracts::validate("coding_start", &value.to_string()).is_err());
    }
    for value in [
        json!({"jobId":"j","cursor":-1}),
        json!({"jobId":"j","limit":101}),
        json!({"jobId":"j","limit":0}),
    ] {
        assert!(contracts::validate("coding_inspect", &value.to_string()).is_err());
    }
}
#[test]
fn coding_single_slot_source_revision_and_cancel_are_enforced() {
    let c = database();
    job(&c);
    assert_eq!(
        repo::source(&c, crate::PRIMARY_CONVERSATION_ID, "host").unwrap(),
        "input"
    );
    assert!(repo::authorize(&c, "job", "unrelated").is_err());
    assert!(repo::revision(&c, "job", 2).is_err());
    assert!(c.execute("INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES('run2','job','input','host','duplicate','digest','prepared','starting','1')",[]).is_err());
    let result = service::cancel(&c, crate::PRIMARY_CONVERSATION_ID, "job", 1, "stop").unwrap();
    assert_eq!(result["state"], "cancel_requested");
    assert_eq!(result["revision"], 2);
    assert!(service::cancel(&c, crate::PRIMARY_CONVERSATION_ID, "job", 1, "stop").is_err());
    assert_eq!(
        service::cancel(&c, crate::PRIMARY_CONVERSATION_ID, "job", 2, "stop").unwrap()["revision"],
        2
    );
    c.execute("DELETE FROM conversation_messages WHERE id='input'", [])
        .unwrap();
    assert!(repo::authorize(&c, "job", crate::PRIMARY_CONVERSATION_ID).is_err());
}
#[test]
fn coding_replay_digest_and_event_cursor_are_bounded() {
    let c = database();
    job(&c);
    c.execute(
        "INSERT INTO coding_calls VALUES('input','call','digest','{\"accepted\":true}')",
        [],
    )
    .unwrap();
    assert_eq!(
        repo::cached(&c, "input", "call", "digest")
            .unwrap()
            .unwrap()["accepted"],
        true
    );
    assert!(repo::cached(&c, "input", "call", "other").is_err());
    for _ in 0..101 {
        repo::event(&c, "job", "run", "test", json!({"text":"日".repeat(200)})).unwrap();
    }
    let page = repo::inspect(&c, crate::PRIMARY_CONVERSATION_ID, "job", 0, 100).unwrap();
    assert!(page.to_string().len() < 32768);
    assert_eq!(page["truncated"], true);
    let next = repo::inspect(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        "job",
        page["nextCursor"].as_u64().unwrap(),
        100,
    )
    .unwrap();
    assert!(next["events"][0]["sequence"].as_u64().unwrap() > page["nextCursor"].as_u64().unwrap());
}
#[test]
fn coding_restart_never_resends_and_blocks_live_or_ambiguous_process() {
    let c = database();
    job(&c);
    super::recovery::reconcile(&c).unwrap();
    assert_eq!(repo::revision(&c, "job", 2).unwrap().0, "interrupted");
    c.execute("UPDATE coding_runs SET state='running',delivery='accepted',pid=?1,process_identity='unverified'",[std::process::id()]).unwrap();
    super::recovery::reconcile(&c).unwrap();
    assert_eq!(repo::revision(&c, "job", 3).unwrap().0, "outcome_unknown");
    let delivery: String = c
        .query_row("SELECT delivery FROM coding_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(delivery, "unknown");
    c.execute(
        "UPDATE coding_runs SET pid=NULL,delivery='prepared',process_identity='launching'",
        [],
    )
    .unwrap();
    super::recovery::reconcile(&c).unwrap();
    assert_eq!(repo::revision(&c, "job", 4).unwrap().0, "outcome_unknown");
}
#[test]
fn coding_dispatch_returns_real_workspace_error_with_default_disabled_settings() {
    let c = database();
    let state = crate::test_support::app_state(c);
    let input = crate::StartTurnInput {
        run_id: "host".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: "implement".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call".into(),
        name: "coding_start".into(),
        arguments: json!({"workspaceId":"invented","request":"implement"}).to_string(),
    };
    assert!(super::tools::execute(Some(&state), &input, &call).contains("coding_disabled"));
    state
        .sqlite_writer
        .write(|c| {
            let mut settings = repo::settings(c)?;
            settings.enabled = true;
            c.execute(
                "UPDATE coding_settings SET value_json=?1",
                params![serde_json::to_string(&settings).unwrap()],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(super::tools::execute(Some(&state), &input, &call).contains("workspace_required"));
}

#[path = "integration/tests.rs"]
mod integration;

#[test]
fn coding_sdk_settings_preserve_legacy_config_and_require_an_explicit_extension() {
    let mut value = serde_json::to_value(contracts::CodingSettings::default()).unwrap();
    value.as_object_mut().unwrap().remove("sdkExtensionPath");
    let mut settings: contracts::CodingSettings = serde_json::from_value(value).unwrap();
    assert!(contracts::valid_profile(&settings));
    settings.profile = "codex-sdk-v1".into();
    settings.provider = "saaa-codex-sdk".into();
    assert!(!contracts::valid_profile(&settings));
    let extension = tempfile::NamedTempFile::new().unwrap();
    settings.sdk_extension_path = Some(extension.path().to_string_lossy().into_owned());
    assert!(contracts::valid_profile(&settings));
    settings.model = "unverified".into();
    assert!(!contracts::valid_profile(&settings));
}

#[path = "e2e/tests.rs"]
mod e2e;
