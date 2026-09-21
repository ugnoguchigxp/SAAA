use super::contracts::{GoalProposal, Notify, Operation, Verifier};
use super::{authority, evidence, intake, verifier};
use crate::persistence::schema::initialize_database;
use crate::PRIMARY_CONVERSATION_ID;
use rusqlite::Connection;

fn db() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    initialize_database(&connection).unwrap();
    connection
}

fn insert_user(connection: &Connection, id: &str, text: &str) {
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES(?1,?2,'user',?3,'1')",
            rusqlite::params![id, PRIMARY_CONVERSATION_ID, text],
        )
        .unwrap();
}

fn proposal(source: &str, ops: Vec<Operation>) -> GoalProposal {
    GoalProposal {
        source_message_id: source.into(),
        workspace_id: "ws".into(),
        summary: "調べる".into(),
        success_condition: Verifier::TestReportObtained,
        operations: ops,
        budget_runs: 1,
        budget_ms: 1000,
        notify: Notify::Silent,
        quote_start: None,
        quote_end: None,
        target: None,
        recipe_id: None,
    }
}

#[test]
fn rf5_v_03_settled_without_evidence_does_not_complete_or_start_next_step() {
    let connection = db();
    connection
        .execute(
            "INSERT INTO coding_workspaces(id,conversation_id,path) VALUES('ws',?1,'/tmp')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    crate::steward::repository::register(
        &connection,
        PRIMARY_CONVERSATION_ID,
        "ws",
        "tests pass",
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,coding_job_id,created_at,updated_at)
             SELECT 'task',d.id,?1,'start','src','k','running','job','1','1' FROM steward_delegations d LIMIT 1",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id)
             VALUES('job',?1,'src','ws','/tmp','{}',1,'/tmp/s','settled','run')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at)
             VALUES('run','job','src','host','x','d','accepted','settled','1')",
            [],
        )
        .unwrap();
    crate::steward::repository::apply_terminal_event(&connection, "job", "settled", Some("run"))
        .unwrap();
    let state: String = connection
        .query_row("SELECT loop_state FROM steward_tasks WHERE id='task'", [], |row| row.get(0))
        .unwrap();
    assert_ne!(state, "done");
    let queued: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM steward_tasks WHERE loop_state='queued'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(queued, 0);
}

#[test]
fn rf5_v_04_missing_is_not_a_success_label() {
    let outcome = verifier::evaluate_evidence(
        "tests_pass",
        None,
        &evidence::from_host_session(
            "t",
            "j",
            "r",
            "settled",
            "/tmp",
            None,
            "",
            None,
            None,
            None,
            None,
            evidence::PRODUCER_HOST_PI,
        ),
    )
    .unwrap();
    assert_eq!(
        outcome.outcome,
        crate::steward::execution_contracts::VerifierOutcome::Missing
    );
}

#[test]
fn rf5_n_01_same_length_edit_invalidates_digest() {
    let a = authority::source_digest("実行して下さい");
    let b = authority::source_digest("実行しないで");
    assert_ne!(a, b);
    assert_eq!(a.len(), 64);
}

#[test]
fn rf5_n_03_and_06_host_intake_uses_full_source() {
    let connection = db();
    insert_user(&connection, "src", "テストを実行しないで");
    let result = intake::classify(
        &connection,
        PRIMARY_CONVERSATION_ID,
        "src",
        &proposal("src", vec![Operation::TestRun]),
    )
    .unwrap();
    assert_eq!(result.reason, "quoted_or_negative_source");
    insert_user(&connection, "ok", "失敗テストをテストして");
    let allowed = intake::classify(
        &connection,
        PRIMARY_CONVERSATION_ID,
        "ok",
        &proposal("ok", vec![Operation::TestRun]),
    )
    .unwrap();
    assert_ne!(allowed.reason, "quoted_or_negative_source");
}

#[test]
fn rf5_g_06_paraphrase_intents_are_requested() {
    use crate::runtime::context::world::query_understand::{understand, QueryUnderstanding};
    use crate::runtime::context::world::question::QuestionIntent;
    let cases = [
        ("Xは今の目標に関係ある？", QuestionIntent::Relevance),
        ("is X related to the current goal", QuestionIntent::Relevance),
        ("Xが変わると何が影響を受ける？", QuestionIntent::Influence),
        ("what would be affected if X changed", QuestionIntent::Influence),
        ("XとYの関連を知りたい", QuestionIntent::Correlation),
        ("how are X and Y related", QuestionIntent::Correlation),
        ("Xの前提は？", QuestionIntent::Dependency),
        ("what does X depend on", QuestionIntent::Dependency),
    ];
    for (text, intent) in cases {
        match understand(text) {
            QueryUnderstanding::Requested(question) => assert_eq!(question.intent, intent, "{text}"),
            other => panic!("{text} => {other:?}"),
        }
    }
}
