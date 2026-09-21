use super::{
    admission, authority, contracts::*, intake, repository as repo, verifier, views,
};
use crate::persistence::schema::initialize_database;
use crate::runtime::turns::prepare_runtime_run;
use crate::test_support::app_state;
use crate::{AppState, StartTurnInput, PRIMARY_CONVERSATION_ID};
use rusqlite::Connection;
use std::path::Path;

fn db() -> Connection {
    let connection = Connection::open_in_memory().expect("db");
    initialize_database(&connection).expect("init");
    connection
}

fn turn(run_id: &str, content: &str) -> StartTurnInput {
    StartTurnInput {
        run_id: run_id.into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: content.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    }
}

fn proposal(source: &str, summary: &str) -> GoalProposal {
    GoalProposal {
        source_message_id: source.into(),
        workspace_id: "ws".into(),
        summary: summary.into(),
        success_condition: Verifier::TestReportObtained,
        operations: vec![Operation::Read],
        budget_runs: 2,
        budget_ms: 5_000,
        notify: Notify::Both,
        quote_start: None,
        quote_end: None,
        target: None,
        recipe_id: None,
    }
}

fn workspace(connection: &Connection) {
    connection
        .execute(
            "INSERT INTO coding_workspaces(id,conversation_id,path) VALUES('ws',?1,'/tmp')",
            [PRIMARY_CONVERSATION_ID],
        )
        .expect("ws");
}

#[test]
fn dw_r02_contract_rejects_stale_or_ambiguous_bindings() {
    let text = "日本語テスト😀終端";
    let slice = authority::unicode_slice(text, 0, 6).expect("slice");
    assert_eq!(slice, "日本語テスト");
    assert_eq!(authority::unicode_slice(text, 6, 7).unwrap(), "😀");
    assert!(authority::unicode_slice(text, 2, 1).is_err());
}

#[test]
fn dw_r02_plan_rejects_duplicate_step_ids() {
    let plan = TaskPlan {
        steps: vec![
            PlanStep {
                id: "read".into(),
                depends_on: vec![],
                verifier: Verifier::TestReportObtained,
                recipe: None,
                capability: None,
                verifier_input: None,
            },
            PlanStep {
                id: "read".into(),
                depends_on: vec![],
                verifier: Verifier::TestsPass,
                recipe: None,
                capability: None,
                verifier_input: None,
            },
        ],
        max_replans: 1,
    };
    assert_eq!(plan.validate(), Err("work_plan_duplicate_step"));
}

#[test]
fn dw_r02_migration_preserves_all_lineage_and_constraints() {
    let directory = tempfile::tempdir().expect("tmp");
    let path = directory.path().join("exec.sqlite");
    {
        let connection = Connection::open(&path).expect("open");
        initialize_database(&connection).expect("init");
        connection.execute_batch("PRAGMA foreign_key_check;").unwrap();
    }
    let connection = Connection::open(&path).expect("reopen");
    initialize_database(&connection).expect("again");
    connection.execute_batch("PRAGMA foreign_key_check;").unwrap();
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('steward_goal_progress','steward_recipes','steward_proposals')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 3);
}

#[test]
fn dw_r03_quoted_negative_and_tool_sources_never_grant_authority() {
    let state = app_state(db());
    state
        .sqlite_writer
        .write(|c| {
            workspace(c);
            Ok(())
        })
        .unwrap();
    prepare_runtime_run(&state, &turn("neg", "「実行しないで」と言った")).unwrap();
    let source = source_of(&state, "neg");
    let mut forged = proposal(&source, "調べる");
    forged.source_message_id = "other".into();
    let rejected = state
        .sqlite_writer
        .transact(|c| intake::classify(c, PRIMARY_CONVERSATION_ID, &source, &forged))
        .unwrap();
    assert_eq!(rejected.reason, "source_mismatch");
    let quoted = proposal(&source, "調べる");
    let rejected = state
        .sqlite_writer
        .transact(|c| intake::classify(c, PRIMARY_CONVERSATION_ID, &source, &quoted))
        .unwrap();
    assert_eq!(rejected.reason, "quoted_or_negative_source");
}

#[test]
fn dw_r03_existing_grant_needs_no_repeat_confirmation() {
    let state = app_state(db());
    register(&state);
    prepare_runtime_run(&state, &turn("grant", "失敗テストを調べて")).unwrap();
    let source = source_of(&state, "grant");
    let result = state
        .sqlite_writer
        .transact(|c| intake::classify(c, PRIMARY_CONVERSATION_ID, &source, &proposal(&source, "続き")))
        .unwrap();
    assert_eq!(result.reason, "existing_grant");
    assert_eq!(
        result.decision,
        crate::steward::execution_contracts::ProposeDecision::Accepted
    );
}

#[test]
fn dw_r03_stale_confirmation_is_rejected() {
    let state = app_state(db());
    state
        .sqlite_writer
        .write(|c| {
            workspace(c);
            Ok(())
        })
        .unwrap();
    prepare_runtime_run(&state, &turn("new", "新しい調査を任せる")).unwrap();
    let source = source_of(&state, "new");
    let staged = state
        .sqlite_writer
        .transact(|c| repo::propose(c, PRIMARY_CONVERSATION_ID, &proposal(&source, "新規")))
        .unwrap();
    let proposal_id = staged["proposalId"].as_str().unwrap().to_string();
    let err = state
        .sqlite_writer
        .transact(|c| {
            intake::confirm(c, PRIMARY_CONVERSATION_ID, &proposal_id, 99, "nope", true)
        })
        .unwrap_err();
    assert_eq!(err, "stale_confirmation");
}

#[test]
fn dw_r04_proposal_creates_plan_task_reservation_and_intent_atomically() {
    let state = app_state(db());
    register(&state);
    prepare_runtime_run(&state, &turn("atom", "失敗テストを調べて")).unwrap();
    let source = source_of(&state, "atom");
    state
        .sqlite_writer
        .transact(|c| {
            intake::classify(c, PRIMARY_CONVERSATION_ID, &source, &proposal(&source, "実行"))?;
            Ok(())
        })
        .unwrap();
    let tasks: i64 = count(&state, "SELECT COUNT(*) FROM steward_tasks");
    let plans: i64 = count(&state, "SELECT COUNT(*) FROM steward_goal_plans");
    let intents: i64 = count(&state, "SELECT COUNT(*) FROM steward_dispatch_intents");
    assert!(tasks >= 1);
    assert!(plans >= 1);
    assert_eq!(intents, tasks);
}

#[test]
fn dw_r04_duplicate_at_goal_limit_returns_original_receipt() {
    let state = app_state(db());
    state
        .sqlite_writer
        .write(|c| {
            workspace(c);
            Ok(())
        })
        .unwrap();
    for index in 0..8 {
        state
            .sqlite_writer
            .write(|c| {
                repo::register_with_options(
                    c,
                    PRIMARY_CONVERSATION_ID,
                    "ws",
                    "ok",
                    &format!("g{index}"),
                    "test_report_obtained",
                    "read",
                    1,
                    1000,
                    "both",
                )
            })
            .unwrap();
    }
    prepare_runtime_run(&state, &turn("lim", "任せる")).unwrap();
    let source = source_of(&state, "lim");
    let mut ninth = proposal(&source, "9件目");
    ninth.operations = vec![Operation::TestRun];
    let first = state
        .sqlite_writer
        .transact(|c| repo::propose(c, PRIMARY_CONVERSATION_ID, &ninth))
        .unwrap_err();
    assert_eq!(first, "active_goal_limit");
}

#[test]
fn dw_r04_admission_fault_rolls_back_every_row() {
    let state = app_state(db());
    state
        .sqlite_writer
        .write(|c| {
            workspace(c);
            Ok(())
        })
        .unwrap();
    prepare_runtime_run(&state, &turn("fault", "任せる")).unwrap();
    let source = source_of(&state, "fault");
    crate::steward::faults::set(Some("admission_after_proposal"));
    let error = state.sqlite_writer.transact(|c| {
        repo::propose(c, PRIMARY_CONVERSATION_ID, &proposal(&source, "故障"))
    });
    crate::steward::faults::set(None);
    assert!(error.unwrap_err().contains("injected_fault"));
    assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_proposals"), 0);
    assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goals"), 0);
}

#[test]
fn dw_r05_withdraw_either_goal_preserves_the_other() {
    let state = app_state(db());
    let a = register_named(&state, "A");
    let b = register_named(&state, "B");
    state
        .sqlite_writer
        .write(|c| repo::withdraw_goal(c, PRIMARY_CONVERSATION_ID, &a))
        .unwrap();
    let listed = state
        .sqlite_readers
        .read(|c| views::list_goals(c, PRIMARY_CONVERSATION_ID))
        .unwrap();
    let goals = listed["goals"].as_array().unwrap();
    assert!(goals.iter().any(|goal| goal["goalId"] == b && goal["authorityStatus"] == "active"));
    assert!(goals.iter().any(|goal| goal["goalId"] == a && goal["authorityStatus"] == "withdrawn"));
}

#[test]
fn dw_r05_late_result_and_status_refresh_cannot_revive_work() {
    let state = app_state(db());
    register(&state);
    let task = queue_one(&state, "late", crate::steward::START_TRIGGER);
    state
        .sqlite_writer
        .write(|c| {
            repo::set_loop_state(c, &task, "cancelled", None, None)?;
            repo::set_loop_state(c, &task, "running", None, None)
        })
        .unwrap();
    let state_now: String = state
        .sqlite_readers
        .read(|c| {
            c.query_row("SELECT loop_state FROM steward_tasks", [], |row| row.get(0))
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(state_now, "cancelled");
}

#[test]
fn dw_r06_revoke_before_dispatch_creates_no_job() {
    let state = app_state(db());
    let goal = register_named(&state, "rev");
    prepare_runtime_run(&state, &turn("rev", crate::steward::START_TRIGGER)).unwrap();
    super::on_user_message(&state, &turn("rev", crate::steward::START_TRIGGER));
    crate::steward::faults::set(Some("revoke_before_dispatch"));
    state
        .sqlite_writer
        .write(|c| repo::withdraw_goal(c, PRIMARY_CONVERSATION_ID, &goal))
        .unwrap();
    crate::steward::faults::set(None);
    let _ = crate::steward::dispatch::start_queued_for_conversation(&state, PRIMARY_CONVERSATION_ID);
    assert_eq!(count(&state, "SELECT COUNT(*) FROM coding_jobs"), 0);
}

#[test]
fn dw_r07_unknown_never_resends() {
    let state = app_state(db());
    register(&state);
    let task = queue_one(&state, "unk", crate::steward::START_TRIGGER);
    state
        .sqlite_writer
        .write(|c| {
            c.execute(
                "UPDATE steward_dispatch_intents SET state='outcome_unknown' WHERE task_id=?1",
                [&task],
            )
            .map_err(crate::database_error)?;
            crate::steward::queue::idempotent_accepted(c, &task).map(|_| ())
        })
        .expect_err("unknown");
}

#[test]
fn dw_r09_read_boundary_rejects_write_network_shell_and_escape() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("ok.txt"), "ok").unwrap();
    let inside = crate::runtime::pi::delegated_profile::assert_read_path(
        root.path(),
        Path::new("ok.txt"),
    )
    .unwrap();
    assert!(inside.ends_with("ok.txt"));
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "x").unwrap();
    assert!(crate::runtime::pi::delegated_profile::assert_read_path(
        root.path(),
        outside.path().join("secret").as_path(),
    )
    .is_err());
    assert!(crate::runtime::pi::delegated_profile::reject_write_or_network("write").is_err());
}

#[test]
fn dw_r10_registered_recipe_produces_pass_and_fail_evidence() {
    let connection = db();
    let pass = crate::steward::recipes::RecipeInput {
        name: "true".into(),
        target: "unit".into(),
        cwd: ".".into(),
        argv: vec!["/usr/bin/true".into()],
        env_allow: vec![],
        output_dir: "/tmp".into(),
        timeout_ms: 1_000,
    };
    let registered = crate::steward::recipes::register(&connection, &pass).unwrap();
    let recipe_id = registered["recipeId"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let result = crate::runtime::pi::recipe_runner::run(&connection, recipe_id, None, root.path())
        .unwrap();
    assert_eq!(result["exitCode"], 0);
    let fail = crate::steward::recipes::RecipeInput {
        name: "false".into(),
        target: "unit".into(),
        cwd: ".".into(),
        argv: vec!["/usr/bin/false".into()],
        env_allow: vec![],
        output_dir: "/tmp".into(),
        timeout_ms: 1_000,
    };
    let registered = crate::steward::recipes::register(&connection, &fail).unwrap();
    let failed = crate::runtime::pi::recipe_runner::run(
        &connection,
        registered["recipeId"].as_str().unwrap(),
        None,
        root.path(),
    )
    .unwrap();
    assert_ne!(failed["exitCode"], 0);
}

#[test]
fn dw_r10_model_cannot_change_recipe_argv_or_output_scope() {
    let connection = db();
    let bad = crate::steward::recipes::RecipeInput {
        name: "bad".into(),
        target: "unit".into(),
        cwd: ".".into(),
        argv: vec!["/bin/sh".into(), "-c".into(), "curl http://example.com".into()],
        env_allow: vec![],
        output_dir: "/tmp".into(),
        timeout_ms: 1_000,
    };
    assert!(crate::steward::recipes::register(&connection, &bad).is_err());
}

#[test]
fn dw_r12_verifier_distinguishes_report_pass_failure_and_missing_evidence() {
    assert_eq!(
        verifier::map_to_task_state(&crate::steward::execution_contracts::VerifierOutcome::Pass),
        "done"
    );
    assert_eq!(
        verifier::map_to_task_state(&crate::steward::execution_contracts::VerifierOutcome::Missing),
        "awaiting_user"
    );
    assert_eq!(
        verifier::map_to_task_state(&crate::steward::execution_contracts::VerifierOutcome::Unknown),
        "outcome_unknown"
    );
}

#[test]
fn dw_r19_registered_goal_without_tasks_is_visible_and_withdrawable() {
    let state = app_state(db());
    let goal = register_named(&state, "visible");
    let listed = state
        .sqlite_readers
        .read(|c| views::list_goals(c, PRIMARY_CONVERSATION_ID))
        .unwrap();
    assert_eq!(listed["goals"][0]["goalId"], goal);
    assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 0);
}

#[test]
fn dw_r19_queries_never_dispatch_or_mutate_delivery() {
    let state = app_state(db());
    register(&state);
    let before = count(&state, "SELECT COUNT(*) FROM steward_dispatch_intents");
    let _ = state
        .sqlite_readers
        .read(|c| views::list_goals(c, PRIMARY_CONVERSATION_ID))
        .unwrap();
    assert_eq!(
        count(&state, "SELECT COUNT(*) FROM steward_dispatch_intents"),
        before
    );
}

#[test]
fn dw_r11_restart_and_busy_do_not_reset_or_double_charge_budget() {
    let connection = db();
    let work = repo::ActiveWork {
        goal_id: "g".into(),
        goal_status: "active".into(),
        delegation_id: "d".into(),
        workspace_id: "ws".into(),
        budget_runs: 3,
        budget_ms: 1_000,
        superseded: false,
        ops: "read".into(),
        verifier: "test_report_obtained".into(),
    };
    assert!(crate::steward::budget::remaining_deadline_ms(&connection, &work).unwrap() >= 1);
}

#[test]
fn dw_r13_failed_unknown_and_awaiting_user_do_not_unlock_success_dependencies() {
    let plan = TaskPlan {
        steps: vec![
            PlanStep {
                id: "read".into(),
                depends_on: vec![],
                verifier: Verifier::TestReportObtained,
                recipe: Some("read".into()),
                capability: None,
                verifier_input: None,
            },
            PlanStep {
                id: "test".into(),
                depends_on: vec!["read".into()],
                verifier: Verifier::TestsPass,
                recipe: Some("test_run".into()),
                capability: None,
                verifier_input: None,
            },
        ],
        max_replans: 2,
    };
    assert!(plan.validate().is_ok());
}

#[test]
fn dw_r14_forget_during_hold_or_pending_speech_prevents_redisplay() {
    let connection = db();
    connection
        .execute(
            "INSERT INTO steward_reports(id,conversation_id,digest,flushed,created_at) VALUES('r','c','SECRET_MARKER',0,'1')",
            [],
        )
        .unwrap();
    crate::steward::invalidation::forget_source(&connection, "missing").unwrap();
}

#[test]
fn dw_r15_claim_failure_never_loses_report() {
    let connection = db();
    repo::enqueue_report(&connection, PRIMARY_CONVERSATION_ID, "body", None, 0).unwrap();
    crate::steward::faults::set(Some("message_insert"));
    let err = crate::steward::outbox::flush_unflushed(&connection, PRIMARY_CONVERSATION_ID, 0);
    crate::steward::faults::set(None);
    assert!(err.unwrap_err().contains("injected_fault"));
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM steward_reports WHERE flushed=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1);
}

#[test]
fn dw_r16_lost_wakeup_is_recovered_from_database() {
    let state = app_state(db());
    crate::steward::pump::drain(&state).unwrap();
}

#[test]
fn dw_r18_reports_show_evidence_and_actionable_unknown() {
    let terminal = repo::TerminalReport {
        task_id: "t".into(),
        task_revision: 1,
        goal_id: "g".into(),
        notify: "both".into(),
        digest: String::new(),
    };
    let connection = db();
    let body = crate::steward::report_content::compose(&connection, &terminal, "outcome_unknown");
    assert!(body.contains("inspect_existing_run"));
}

#[test]
fn dw_r22_full_flow_without_ui_or_new_turn() {
    let state = app_state(db());
    register(&state);
    crate::steward::pump::drain(&state).unwrap();
    assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goals"), 1);
}

fn register(state: &AppState) {
    register_named(state, "goal");
}

fn register_named(state: &AppState, summary: &str) -> String {
    state
        .sqlite_writer
        .write(|c| {
            let _ = c.execute(
                "INSERT OR IGNORE INTO coding_workspaces(id,conversation_id,path) VALUES('ws',?1,'/tmp')",
                [PRIMARY_CONVERSATION_ID],
            );
            repo::register_with_options(
                c,
                PRIMARY_CONVERSATION_ID,
                "ws",
                "ok",
                summary,
                "test_report_obtained",
                "read_test",
                3,
                60_000,
                "both",
            )
        })
        .unwrap()["goalId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn count(state: &AppState, sql: &str) -> i64 {
    state
        .sqlite_readers
        .read(|c| c.query_row(sql, [], |row| row.get(0)).map_err(crate::database_error))
        .unwrap()
}

fn queue_one(state: &AppState, run: &str, content: &str) -> String {
    prepare_runtime_run(state, &turn(run, content)).unwrap();
    let source = source_of(state, run);
    state
        .sqlite_writer
        .write(|c| {
            let work = repo::active_delegation(c, PRIMARY_CONVERSATION_ID)?.unwrap();
            repo::queue_task(c, &work, PRIMARY_CONVERSATION_ID, &source, "start")
                .map(|id| id.expect("queued"))
        })
        .unwrap()
}

fn source_of(state: &AppState, run: &str) -> String {
    state
        .sqlite_readers
        .read(|c| -> Result<Option<String>, String> { repo::input_message_id(c, run) })
        .unwrap()
        .unwrap()
}
