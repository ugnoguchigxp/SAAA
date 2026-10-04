use super::*;
use crate::coding::{repository as repo, service};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
pub(super) fn fixture() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute("INSERT INTO conversation_messages VALUES('human',?1,'user','Use Blue, and run bun test','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let settings = crate::coding::contracts::CodingSettings {
        enabled: true,
        implementation_method: "terminal".into(),
        terminal_cli: "claude".into(),
        terminal_kind: "kitty".into(),
        terminal_auto_answer: true,
        ..Default::default()
    };
    c.execute("INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id,session_id) VALUES('job',?1,'human','workspace','/tmp',?2,1,'/tmp/terminal-fixture','running','run','exact-session')",params![crate::PRIMARY_CONVERSATION_ID,serde_json::to_string(&settings).unwrap()]).unwrap();
    c.execute("INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES('run','job','human','host','Use Blue','digest','accepted','running','1')",[]).unwrap();
    c.execute("INSERT INTO terminal_runs(run_id,directory,nonce,phase,cli,terminal,baseline_json,checks_json,created_at) VALUES('run','/tmp/private-fixture','nonce','running','claude','kitty','{}','[]','1')",[]).unwrap();
    c
}
fn event(id: &str, kind: &str, data: Value) -> saaa_terminal_agent_runtime::Event {
    saaa_terminal_agent_runtime::Event {
        id: id.into(),
        run: "run".into(),
        nonce: "nonce".into(),
        kind: kind.into(),
        data,
    }
}
fn pause(c: &Connection, kind: &str) {
    ingress::consume_test(
        c,
        &event(
            "event",
            "question",
            json!({"questionId":"q","kind":kind,"input":{"question":"Which color?"}}),
        ),
        100,
    )
    .unwrap();
    c.execute(
        "UPDATE terminal_runs SET phase='paused' WHERE run_id='run'",
        [],
    )
    .unwrap();
    c.execute("UPDATE coding_runs SET state='settled' WHERE id='run'", [])
        .unwrap();
    c.execute(
        "UPDATE terminal_questions SET state='awaiting_user' WHERE id='run:q'",
        [],
    )
    .unwrap();
}
#[test]
fn duplicate_events_do_not_repeat_questions_or_increment_revision() {
    let c = fixture();
    let e = event(
        "event",
        "question",
        json!({"questionId":"q","kind":"blocker","input":{"question":"Which color?"}}),
    );
    assert!(ingress::consume_test(&c, &e, 100).unwrap());
    assert!(!ingress::consume_test(&c, &e, 100).unwrap());
    assert_eq!(
        c.query_row("SELECT revision FROM coding_jobs", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        ledger::context(&c, "job").unwrap()["questions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn pending_questions_cannot_resume_before_exit() {
    let c = fixture();
    ingress::consume_test(
        &c,
        &event(
            "event",
            "question",
            json!({"questionId":"q","kind":"blocker","input":{"question":"Which color?"}}),
        ),
        100,
    )
    .unwrap();
    assert!(answer(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        "job",
        2,
        "run:q",
        &json!("Blue"),
        Some("human"),
        "decision"
    )
    .is_err());
    assert_eq!(
        c.query_row("SELECT count(*) FROM coding_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn decisions_are_event_origins_and_do_not_fabricate_human_turns() {
    let c = fixture();
    pause(&c, "blocker");
    let run = answer(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        "job",
        2,
        "run:q",
        &json!("Blue"),
        None,
        "decision",
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM conversation_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(repo::authorize(&c, "job", crate::PRIMARY_CONVERSATION_ID).is_ok());
    let source: String = c
        .query_row(
            "SELECT source_id FROM coding_runs WHERE id=?1",
            [run],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ledger::source_authorized(&c, &source, "job").unwrap());
    c.execute("DELETE FROM conversation_messages WHERE id='human'", [])
        .unwrap();
    assert!(repo::authorize(&c, "job", crate::PRIMARY_CONVERSATION_ID).is_err());
}
#[test]
fn automatic_permission_and_cross_conversation_answers_are_rejected() {
    let c = fixture();
    pause(&c, "permission");
    assert_eq!(
        answer(
            &c,
            crate::PRIMARY_CONVERSATION_ID,
            "job",
            2,
            "run:q",
            &json!("approve"),
            None,
            "decision"
        )
        .unwrap_err(),
        "permission_requires_user"
    );
    assert!(answer(
        &c,
        "unrelated",
        "job",
        2,
        "run:q",
        &json!("approve"),
        Some("human"),
        "decision"
    )
    .is_err());
}
#[test]
fn answer_and_cancel_enforce_revisions_and_confirm_paused_stop() {
    let c = fixture();
    pause(&c, "blocker");
    assert!(answer(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        "job",
        1,
        "run:q",
        &json!("Blue"),
        Some("human"),
        "decision"
    )
    .is_err());
    let stopped = service::cancel(&c, crate::PRIMARY_CONVERSATION_ID, "job", 2, "stop").unwrap();
    assert_eq!(stopped["state"], "interrupted");
    assert_eq!(
        c.query_row("SELECT state FROM coding_runs", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "settled"
    );
    assert!(answer(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        "job",
        3,
        "run:q",
        &json!("Blue"),
        Some("human"),
        "decision"
    )
    .is_err());
}
#[test]
fn native_exit_does_not_mark_goal_completed() {
    let c = fixture();
    ingress::consume_test(
        &c,
        &event("exit", "exit", json!({"code":0,"stopped":false})),
        100,
    )
    .unwrap();
    assert_eq!(
        c.query_row("SELECT state FROM coding_jobs", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "running"
    );
    assert_eq!(
        c.query_row("SELECT phase FROM terminal_runs", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "exited"
    );
}
#[test]
fn recovery_leaves_independent_terminal_runner_for_spool_reconciliation() {
    let c = fixture();
    crate::coding::recovery::reconcile(&c).unwrap();
    assert_eq!(
        c.query_row("SELECT state FROM coding_runs", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "running"
    );
}
#[test]
fn old_run_output_cannot_change_current_session() {
    let c = fixture();
    c.execute("UPDATE coding_jobs SET current_run_id='new-run'", [])
        .unwrap();
    assert!(!ingress::consume_test(
        &c,
        &event(
            "output",
            "output",
            json!({"type":"system","session_id":"wrong-session"})
        ),
        100
    )
    .unwrap());
    assert_eq!(
        c.query_row("SELECT session_id FROM coding_jobs", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "exact-session"
    );
}
#[test]
fn mismatched_resume_session_rolls_back_inbox_and_offset() {
    let c = fixture();
    let tx = c.unchecked_transaction().unwrap();
    assert!(ingress::consume_test(
        &tx,
        &event("output", "output", json!({"session_id":"wrong-session"})),
        100
    )
    .is_err());
    tx.rollback().unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM terminal_inbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT offset FROM terminal_runs", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn question_jobs_prioritize_real_human_jobs_and_retry_only_once() {
    let mut c = fixture();
    pause(&c, "blocker");
    questions::schedule(&c, "job", crate::PRIMARY_CONVERSATION_ID, "run").unwrap();
    crate::task_queue::enqueue(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        "conversation",
        "user_input",
        "foreground",
        0,
        "{}",
        None,
    )
    .unwrap();
    let job = crate::task_queue::claim(&mut c, "conversation")
        .unwrap()
        .unwrap();
    assert_eq!(job.kind, "user_input");
}
#[test]
fn legacy_settings_are_preserved_and_terminal_defaults_require_selection() {
    let old = json!({"enabled":true,"implementationMethod":"pi","codexModel":"original","executable":"original","version":"0.86.1","provider":"configured-provider","model":"configured-model","profile":"trusted-local-v1","sdkExtensionPath":null});
    let settings: crate::coding::contracts::CodingSettings = serde_json::from_value(old).unwrap();
    assert_eq!(settings.provider, "configured-provider");
    assert!(settings.terminal_kind.is_empty());
    assert!(!validate(&settings));
}

#[test]
fn automatic_decisions_are_bounded_and_permissions_stay_manual() {
    let c = fixture();
    for i in 0..5 {
        c.execute("INSERT INTO terminal_questions(id,run_id,job_id,kind,input_json,state,created_at) VALUES(?1,'run','job',?2,?3,'awaiting_user','1')",params![format!("q{i}"),if i==4{"permission"}else{"blocker"},json!({"question":format!("Q{i}")}).to_string()]).unwrap();
    }
    questions::schedule(&c, "job", crate::PRIMARY_CONVERSATION_ID, "run").unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM terminal_decisions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM terminal_decisions WHERE question_id='q4'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
#[test]
fn orphaned_launch_receipts_never_create_a_second_run() {
    let c = fixture();
    c.execute(
        "UPDATE terminal_runs SET phase='prepared',created_at='2000-01-01T00:00:00Z'",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE coding_runs SET state='starting',delivery='prepared'",
        [],
    )
    .unwrap();
    let state = crate::test_support::app_state(c);
    assert!(recovery::unlaunched(&state.sqlite_writer, "run").unwrap());
    state
        .sqlite_readers
        .read(|c| {
            assert_eq!(
                c.query_row("SELECT state FROM coding_runs", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "failed"
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM coding_runs", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn selected_terminal_settings_survive_database_reopen_without_replacing_other_methods() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let settings = crate::coding::contracts::CodingSettings {
        enabled: true,
        implementation_method: "terminal".into(),
        terminal_kind: "ghostty".into(),
        terminal_cli: "codex".into(),
        terminal_model: "explicit-model".into(),
        terminal_checks: vec![vec!["/usr/bin/true".into()]],
        provider: "saved-provider".into(),
        model: "saved-pi-model".into(),
        codex_model: "saved-sdk-model".into(),
        ..Default::default()
    };
    {
        let c = Connection::open(&path).unwrap();
        crate::persistence::schema::initialize_database(&c).unwrap();
        c.execute(
            "UPDATE coding_settings SET value_json=?1",
            [serde_json::to_string(&settings).unwrap()],
        )
        .unwrap();
    }
    let c = Connection::open(path).unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    let restored = repo::settings(&c).unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&settings).unwrap()
    );
    assert_eq!(restored.provider, "saved-provider");
}
#[test]
fn automatic_answers_require_source_quotes_and_only_record_real_decisions() {
    for (color, expected, reports) in [("Red", "awaiting_user", 1), ("Blue", "answered", 0)] {
        let c = fixture();
        pause(&c, "blocker");
        c.execute("INSERT INTO terminal_decisions(question_id,job_id,status) VALUES('run:q','job','reserved')",[]).unwrap();
        // A second unanswered question keeps the fixture paused; this test never launches a CLI.
        c.execute("INSERT INTO terminal_questions(id,run_id,job_id,kind,input_json,state,created_at) VALUES('run:second','run','job','blocker','{}','awaiting_user','1')",[]).unwrap();
        let state = crate::test_support::app_state(c);
        let context = automatic_context(&state, "run:q").unwrap();
        automatic_answer(
            &state,
            &context,
            &json!({"action":"answer","sourceId":"human","quote":"Use Blue","answer":color}),
            None,
        )
        .unwrap();
        state
            .sqlite_readers
            .read(|c| {
                assert_eq!(
                    c.query_row(
                        "SELECT state FROM terminal_questions WHERE id='run:q'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                    expected
                );
                assert_eq!(
                    c.query_row("SELECT count(*) FROM steward_reports", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    reports
                );
                assert_eq!(
                    c.query_row(
                        "SELECT count(*) FROM conversation_messages WHERE role='user'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    1
                );
                Ok(())
            })
            .unwrap();
    }
}

#[path = "purpose_route_tests.rs"]
mod purpose_route_tests;
