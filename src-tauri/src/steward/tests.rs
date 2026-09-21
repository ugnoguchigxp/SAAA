use super::contracts::{GoalProposal, Notify, Operation, PlanStep, TaskPlan, Verifier};
use super::repository as repo;
use super::{CONTINUE_TRIGGER, START_REQUEST, START_TRIGGER};
use crate::persistence::schema::{initialize_database, DATABASE_SCHEMA_VERSION};
use crate::runtime::turns::prepare_runtime_run;
use crate::situation::contracts::ForegroundCategory;
use crate::situation::speech_holds_tts;
use crate::test_support::app_state;
use crate::{AppState, StartTurnInput, PRIMARY_CONVERSATION_ID};
use rusqlite::{params, Connection};
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_memory(on: bool, run: impl FnOnce()) {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let previous = std::env::var("SAAA_MEMORY_ENABLED").ok();
    if on {
        std::env::set_var("SAAA_MEMORY_ENABLED", "1");
    } else {
        std::env::remove_var("SAAA_MEMORY_ENABLED");
    }
    run();
    match previous {
        Some(value) => std::env::set_var("SAAA_MEMORY_ENABLED", value),
        None => std::env::remove_var("SAAA_MEMORY_ENABLED"),
    }
}

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

fn workspace(connection: &Connection) -> String {
    connection
        .execute(
            "INSERT INTO coding_workspaces(id,conversation_id,path) VALUES('ws',?1,'/tmp')",
            [PRIMARY_CONVERSATION_ID],
        )
        .expect("workspace");
    "ws".into()
}

fn register_goal(state: &AppState) {
    state
        .sqlite_writer
        .write(|connection| {
            let _ = workspace(connection);
            repo::register(connection, PRIMARY_CONVERSATION_ID, "ws", "tests pass")
        })
        .expect("register");
}

fn count(state: &AppState, sql: &str) -> i64 {
    state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(sql, [], |row| row.get(0))
                .map_err(crate::database_error)
        })
        .expect("count")
}

fn insert_job(state: &AppState, task_id: &str, job_state: &str, identity: Option<&str>) {
    state
        .sqlite_writer
        .write(|connection| {
            let source: String = connection
                .query_row(
                    "SELECT source_id FROM steward_tasks WHERE id=?1",
                    [task_id],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)?;
            connection
                .execute(
                    "INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id)
                     VALUES('job',?1,?2,'ws','/tmp','{}',1,'/tmp/session',?3,'run')",
                    params![PRIMARY_CONVERSATION_ID, source, job_state],
                )
                .map_err(crate::database_error)?;
            connection
                .execute(
                    "INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at,process_identity)
                     VALUES('run','job',?1,'host','inspect','digest','prepared',?2,'1',?3)",
                    params![source, job_state, identity],
                )
                .map_err(crate::database_error)?;
            repo::set_loop_state(connection, task_id, "running", Some("job"), None)?;
            super::evidence::persist(
                connection,
                &super::evidence::from_host_session(
                    task_id,
                    "job",
                    "run",
                    job_state,
                    "/tmp",
                    Some("report"),
                    "host report",
                    Some("read"),
                    Some(1),
                    Some("digest"),
                    Some(0),
                    super::evidence::PRODUCER_HOST_RECIPE,
                ),
            )?;
            Ok(())
        })
        .expect("job");
}

fn task_id(state: &AppState) -> String {
    state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row("SELECT id FROM steward_tasks LIMIT 1", [], |row| row.get(0))
                .map_err(crate::database_error)
        })
        .expect("task")
}

#[test]
fn ml_00_filter_runs() {
    assert!(!START_TRIGGER.is_empty());
}

#[test]
fn ml_01_schema_version_and_empty_goals() {
    let connection = db();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, DATABASE_SCHEMA_VERSION);
    assert_eq!(DATABASE_SCHEMA_VERSION, 31);
    let goals: i64 = connection
        .query_row("SELECT COUNT(*) FROM steward_goals", [], |row| row.get(0))
        .expect("goals");
    assert_eq!(goals, 0);
    connection
        .execute_batch(
            "DROP TABLE steward_reports;
             DROP TABLE steward_runtime;
             DROP TABLE steward_tasks;
             DROP TABLE steward_delegations;
             DROP TABLE steward_goals;",
        )
        .expect("drop");
    connection
        .pragma_update(None, "user_version", 25)
        .expect("downgrade");
    initialize_database(&connection).expect("reopen");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, DATABASE_SCHEMA_VERSION);
}

#[test]
fn ml_01_status_changes_insert_not_overwrite() {
    let state = app_state(db());
    register_goal(&state);
    state
        .sqlite_writer
        .write(|connection| repo::withdraw(connection, PRIMARY_CONVERSATION_ID))
        .expect("withdraw");
    let active: String = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT status FROM steward_goals WHERE superseded_by IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .expect("old");
    assert_eq!(active, "active");
    assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goals"), 2);
}

#[test]
fn ml_01_invalid_ops_rejected() {
    let connection = db();
    workspace(&connection);
    connection
        .execute(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at)
             VALUES('g',?1,'user_explicit','x','active','1')",
            [PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    let error = connection.execute(
        "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at)
         VALUES('d','g',?1,'ws','write',1,1,'both','active','1')",
        [PRIMARY_CONVERSATION_ID],
    );
    assert!(error.is_err());
}

#[test]
fn dw_04_direct_registration_persists_only_the_confirmed_bounded_scope() {
    let connection = db();
    workspace(&connection);
    let registered = repo::register_with_options(
        &connection,
        PRIMARY_CONVERSATION_ID,
        "ws",
        "指定テストの結果を取得する",
        "失敗テストを調査する",
        "test_report_obtained",
        "test_run",
        2,
        30_000,
        "silent",
    )
    .expect("register bounded scope");
    let goal = registered["goalId"].as_str().expect("goal id");
    let saved: (String, String, i64, i64, String, String) = connection
        .query_row(
            "SELECT g.summary,d.ops,d.budget_runs,d.budget_ms,d.notify,g.verifier
             FROM steward_goals g JOIN steward_delegations d ON d.goal_id=g.id WHERE g.id=?1",
            [goal],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("saved scope");
    assert_eq!(
        saved,
        (
            "失敗テストを調査する".into(),
            "test_run".into(),
            2,
            30_000,
            "silent".into(),
            "test_report_obtained".into()
        )
    );
    assert!(repo::register_with_options(
        &connection,
        PRIMARY_CONVERSATION_ID,
        "ws",
        "x",
        "",
        "tests_pass",
        "write",
        1,
        1,
        "both"
    )
    .is_err());
}

#[test]
fn ml_02_register_requires_workspace() {
    let state = app_state(db());
    let error = state.sqlite_writer.write(|connection| {
        repo::register(connection, PRIMARY_CONVERSATION_ID, "ws", "tests pass")
    });
    assert_eq!(error.unwrap_err(), "workspace_required");
}

#[test]
fn dw_01_multiple_active_goals_are_allowed_and_no_delete() {
    let state = app_state(db());
    register_goal(&state);
    let second = state
        .sqlite_writer
        .write(|connection| repo::register(connection, PRIMARY_CONVERSATION_ID, "ws", "again"));
    assert!(second.is_ok());
    assert_eq!(
        count(
            &state,
            "SELECT COUNT(*) FROM steward_goals WHERE status='active'"
        ),
        2
    );
    let sources = [
        include_str!("schema.rs"),
        include_str!("repository.rs"),
        include_str!("reduce.rs"),
        include_str!("report.rs"),
        include_str!("commands.rs"),
    ];
    for source in sources {
        for line in source.lines() {
            if line.contains("DELETE FROM") && line.contains("steward") && !line.contains("forget")
            {
                panic!("steward must not delete rows: {line}");
            }
        }
    }
}

#[test]
fn dw_01_direct_registration_allows_eight_goals_then_enforces_the_limit() {
    let connection = db();
    workspace(&connection);
    for index in 0..8 {
        let registered = repo::register(
            &connection,
            PRIMARY_CONVERSATION_ID,
            "ws",
            &format!("goal {index}"),
        );
        assert!(registered.is_ok());
    }
    assert_eq!(
        repo::register(&connection, PRIMARY_CONVERSATION_ID, "ws", "one too many"),
        Err("active_goal_limit".into())
    );
}

#[test]
fn dw_01_proposal_rejects_ambiguous_or_unbounded_authority() {
    let valid = GoalProposal {
        source_message_id: "message".into(),
        workspace_id: "ws".into(),
        summary: "調べる".into(),
        success_condition: Verifier::TestReportObtained,
        operations: vec![Operation::Read, Operation::TestRun],
        budget_runs: 1,
        budget_ms: 1_000,
        notify: Notify::Both,
        quote_start: None,
        quote_end: None,
        target: None,
        recipe_id: None,
    };
    assert!(valid.validate().is_ok());
    let mut invalid = valid.clone();
    invalid.operations.clear();
    assert_eq!(invalid.validate(), Err("work_proposal_invalid"));
    invalid = valid;
    invalid.budget_runs = 17;
    assert_eq!(invalid.validate(), Err("work_proposal_invalid"));
}

#[test]
fn dw_01_plan_rejects_cycles_and_replan_limit() {
    let plan = TaskPlan {
        steps: vec![PlanStep {
            id: "inspect".into(),
            depends_on: Vec::new(),
            verifier: Verifier::TestReportObtained,
            recipe: None,
            capability: None,
            verifier_input: None,
        }],
        max_replans: 2,
    };
    assert!(plan.validate().is_ok());
    let cycle = TaskPlan {
        steps: vec![
            PlanStep {
                id: "a".into(),
                depends_on: vec!["b".into()],
                verifier: Verifier::TestReportObtained,
                recipe: None,
                capability: None,
                verifier_input: None,
            },
            PlanStep {
                id: "b".into(),
                depends_on: vec!["a".into()],
                verifier: Verifier::TestReportObtained,
                recipe: None,
                capability: None,
                verifier_input: None,
            },
        ],
        max_replans: 0,
    };
    assert_eq!(cycle.validate(), Err("work_plan_cycle"));
    let over_limit = TaskPlan {
        steps: plan.steps,
        max_replans: 3,
    };
    assert_eq!(over_limit.validate(), Err("work_plan_invalid"));
}

#[test]
fn dw_10_settled_read_step_durably_enqueues_one_dependent_test_step() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goal_plans"), 1);
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_plan_steps"), 2);
        prepare_runtime_run(&state, &turn("dw-10-start", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("dw-10-start", START_TRIGGER));
        let first = task_id(&state);
        state
            .sqlite_writer
            .write(|connection| {
                repo::persist_task_plan(connection, &first, "read", "inspect", "rules", 0)
            })
            .unwrap();
        insert_job(&state, &first, "running", None);
        state
            .sqlite_writer
            .write(|connection| repo::apply_terminal_event(connection, "job", "settled", Some("run")))
            .unwrap();
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM steward_task_artifacts WHERE reference='job'"
            ),
            1
        );
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 2);
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM steward_tasks WHERE loop_state='queued'"
            ),
            1
        );
        let successor_step: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT plan_step_id FROM steward_tasks WHERE id != ?1",
                        [&first],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(successor_step, "test");
        state
            .sqlite_writer
            .write(|connection| repo::apply_terminal_event(connection, "job", "settled", Some("run")))
            .unwrap();
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 2);
    });
}

#[test]
fn dw_10_migration_backfills_a_plan_for_an_existing_goal() {
    let connection = db();
    workspace(&connection);
    repo::register(&connection, PRIMARY_CONVERSATION_ID, "ws", "tests pass").unwrap();
    connection
        .execute_batch("DROP TABLE steward_plan_steps; DROP TABLE steward_goal_plans;")
        .unwrap();
    super::schema::migrate(&connection).unwrap();
    let plans: i64 = connection
        .query_row("SELECT COUNT(*) FROM steward_goal_plans", [], |row| {
            row.get(0)
        })
        .unwrap();
    let steps: i64 = connection
        .query_row("SELECT COUNT(*) FROM steward_plan_steps", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(plans, 1);
    assert_eq!(steps, 2);
}

#[test]
fn dw_10_failure_creates_at_most_two_durable_replans() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("dw-10-replan", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("dw-10-replan", START_TRIGGER));
        let first = task_id(&state);
        insert_job(&state, &first, "running", None);
        state
            .sqlite_writer
            .write(|connection| repo::apply_terminal_event(connection, "job", "failed", Some("run")))
            .unwrap();
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goal_plans"), 2);
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 2);
        let revision: i64 = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row("SELECT MAX(revision) FROM steward_goal_plans", [], |row| {
                        row.get(0)
                    })
                    .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(revision, 2);
        let second: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT id FROM steward_tasks WHERE id != ?1",
                        [&first],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute("DELETE FROM coding_runs", [])
                    .map_err(crate::database_error)?;
                connection
                    .execute("DELETE FROM coding_jobs", [])
                    .map_err(crate::database_error)?;
                connection
                    .execute(
                        "UPDATE steward_tasks SET coding_job_id=NULL WHERE id=?1",
                        [&first],
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        insert_job(&state, &second, "running", None);
        state
            .sqlite_writer
            .write(|connection| repo::apply_terminal_event(connection, "job", "failed", Some("run")))
            .unwrap();
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goal_plans"), 3);
        let third: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT id FROM steward_tasks WHERE id NOT IN (?1,?2)",
                        params![first, second],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute("DELETE FROM coding_runs", [])
                    .map_err(crate::database_error)?;
                connection
                    .execute("DELETE FROM coding_jobs", [])
                    .map_err(crate::database_error)?;
                connection
                    .execute(
                        "UPDATE steward_tasks SET coding_job_id=NULL WHERE id=?1",
                        [&second],
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        insert_job(&state, &third, "running", None);
        state
            .sqlite_writer
            .write(|connection| repo::apply_terminal_event(connection, "job", "failed", Some("run")))
            .unwrap();
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goal_plans"), 3);
    });
}

#[test]
fn dw_03_proposal_binds_only_a_persisted_user_source_and_allows_multiple_goals() {
    let state = app_state(db());
    state
        .sqlite_writer
        .write(|c| {
            workspace(c);
            Ok(())
        })
        .unwrap();
    prepare_runtime_run(&state, &turn("proposal-source", "失敗テストを調べて")).unwrap();
    let source: String = state
        .sqlite_readers
        .read(|c| repo::input_message_id(c, "proposal-source"))
        .unwrap()
        .unwrap();
    prepare_runtime_run(&state, &turn("proposal-source-b", "別の失敗テストを調べて")).unwrap();
    let source_b: String = state
        .sqlite_readers
        .read(|c| repo::input_message_id(c, "proposal-source-b"))
        .unwrap()
        .unwrap();
    let proposal = |summary: &str, source_message_id: String| GoalProposal {
        source_message_id,
        workspace_id: "ws".into(),
        summary: summary.into(),
        success_condition: Verifier::TestReportObtained,
        operations: vec![Operation::Read, Operation::TestRun],
        budget_runs: 1,
        budget_ms: 1_000,
        notify: Notify::Both,
        quote_start: None,
        quote_end: None,
        target: None,
        recipe_id: None,
    };
    state
        .sqlite_writer
        .write(|c| repo::propose(c, PRIMARY_CONVERSATION_ID, &proposal("A", source.clone())))
        .unwrap();
    let duplicate = state
        .sqlite_writer
        .write(|c| {
            repo::propose(
                c,
                PRIMARY_CONVERSATION_ID,
                &proposal("A again", source.clone()),
            )
        })
        .unwrap();
    assert_eq!(duplicate["duplicate"], true);
    assert!(
        duplicate["requiresConfirmation"] == true
            || duplicate["decision"] == "requires_confirmation"
            || duplicate["decision"] == "accepted"
    );
    state
        .sqlite_writer
        .write(|c| repo::propose(c, PRIMARY_CONVERSATION_ID, &proposal("B", source_b)))
        .unwrap();
    let proposals: i64 = count(&state, "SELECT COUNT(*) FROM steward_proposals");
    assert_eq!(proposals, 2);
    let mut forged = proposal("forged", source);
    forged.source_message_id = "missing".into();
    let error = state
        .sqlite_writer
        .write(|c| repo::propose(c, PRIMARY_CONVERSATION_ID, &forged));
    assert_eq!(error.unwrap_err(), "source_unavailable");
}

#[test]
fn dw_02_reservation_is_durable_and_cancelled_before_dispatch_is_released() {
    let state = app_state(db());
    register_goal(&state);
    prepare_runtime_run(&state, &turn("reserve-source", START_TRIGGER)).unwrap();
    let source = state
        .sqlite_readers
        .read(|c| repo::input_message_id(c, "reserve-source"))
        .unwrap()
        .unwrap();
    let task = state
        .sqlite_writer
        .write(|c| {
            let work = repo::active_delegation(c, PRIMARY_CONVERSATION_ID)?.unwrap();
            repo::queue_task(c, &work, PRIMARY_CONVERSATION_ID, &source, "start")
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        count(
            &state,
            "SELECT COUNT(*) FROM steward_budget_reservations WHERE state='reserved'"
        ),
        1
    );
    state
        .sqlite_writer
        .write(|c| repo::set_loop_state(c, &task, "cancelled", None, None))
        .unwrap();
    assert_eq!(
        count(
            &state,
            "SELECT COUNT(*) FROM steward_budget_reservations WHERE state='released'"
        ),
        1
    );
}

#[test]
fn dw_06_dispatch_intent_is_claimed_once_and_records_receipt() {
    let state = app_state(db());
    register_goal(&state);
    prepare_runtime_run(&state, &turn("intent-source", START_TRIGGER)).unwrap();
    let source = state
        .sqlite_readers
        .read(|c| repo::input_message_id(c, "intent-source"))
        .unwrap()
        .unwrap();
    let task = state
        .sqlite_writer
        .write(|c| {
            let work = repo::active_delegation(c, PRIMARY_CONVERSATION_ID)?.unwrap();
            repo::queue_task(c, &work, PRIMARY_CONVERSATION_ID, &source, "start")
        })
        .unwrap()
        .unwrap();
    state
        .sqlite_writer
        .write(|c| {
            assert!(repo::claim_dispatch(c, &task)?);
            assert!(!repo::claim_dispatch(c, &task)?);
            repo::settle_dispatch(c, &task, Some(&serde_json::json!({"accepted":true})), false)
        })
        .unwrap();
    assert_eq!(
        count(
            &state,
            "SELECT COUNT(*) FROM steward_dispatch_intents WHERE state='accepted'"
        ),
        1
    );
}

#[test]
fn dw_06_restart_marks_unreceived_dispatch_unknown_without_reclaiming() {
    let state = app_state(db());
    register_goal(&state);
    prepare_runtime_run(&state, &turn("restart-intent-source", START_TRIGGER)).unwrap();
    let source = state
        .sqlite_readers
        .read(|c| repo::input_message_id(c, "restart-intent-source"))
        .unwrap()
        .unwrap();
    let task = state
        .sqlite_writer
        .write(|c| {
            let work = repo::active_delegation(c, PRIMARY_CONVERSATION_ID)?.unwrap();
            repo::queue_task(c, &work, PRIMARY_CONVERSATION_ID, &source, "start")
        })
        .unwrap()
        .unwrap();
    state
        .sqlite_writer
        .write(|c| {
            assert!(repo::claim_dispatch(c, &task)?);
            // This is the startup migration path: the process may have
            // received the effect, so it is intentionally not replayed.
            super::schema::migrate(c).map_err(crate::database_error)
        })
        .unwrap();
    let intent: String = state
        .sqlite_readers
        .read(|c| {
            c.query_row(
                "SELECT state FROM steward_dispatch_intents WHERE task_id=?1",
                [&task],
                |row| row.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(intent, "outcome_unknown");
    assert!(!state
        .sqlite_writer
        .write(|c| repo::claim_dispatch(c, &task))
        .unwrap());
    assert_eq!(count(&state, "SELECT COUNT(*) FROM coding_jobs"), 0);
}

#[test]
fn dw_13_forgetting_source_cancels_derived_task() {
    let state = app_state(db());
    register_goal(&state);
    prepare_runtime_run(&state, &turn("forget-source", START_TRIGGER)).unwrap();
    let source = state
        .sqlite_readers
        .read(|c| repo::input_message_id(c, "forget-source"))
        .unwrap()
        .unwrap();
    let task = state
        .sqlite_writer
        .write(|c| {
            let work = repo::active_delegation(c, PRIMARY_CONVERSATION_ID)?.unwrap();
            repo::queue_task(c, &work, PRIMARY_CONVERSATION_ID, &source, "start")
        })
        .unwrap()
        .unwrap();
    state
        .sqlite_writer
        .write(|c| repo::forget_source(c, &source))
        .unwrap();
    let state_name: String = state
        .sqlite_readers
        .read(|c| {
            c.query_row(
                "SELECT loop_state FROM steward_tasks WHERE id=?1",
                [&task],
                |r| r.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(state_name, "cancelled");
}

#[test]
fn dw_13_forgetting_source_requests_stop_for_running_coding_job() {
    let state = app_state(db());
    register_goal(&state);
    prepare_runtime_run(&state, &turn("forget-running", START_TRIGGER)).unwrap();
    let source = state
        .sqlite_readers
        .read(|c| repo::input_message_id(c, "forget-running"))
        .unwrap()
        .unwrap();
    let task = state
        .sqlite_writer
        .write(|c| {
            let work = repo::active_delegation(c, PRIMARY_CONVERSATION_ID)?.unwrap();
            repo::queue_task(c, &work, PRIMARY_CONVERSATION_ID, &source, "start")
        })
        .unwrap()
        .unwrap();
    insert_job(&state, &task, "running", None);
    state
        .sqlite_writer
        .write(|c| repo::forget_source(c, &source))
        .unwrap();
    let run_state: String = state
        .sqlite_readers
        .read(|c| {
            c.query_row("SELECT state FROM coding_runs WHERE id='run'", [], |r| {
                r.get(0)
            })
            .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(run_state, "stopping");
}

#[test]
fn dw_13_late_terminal_event_cannot_revive_cancelled_task() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("late-source", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("late-source", START_TRIGGER));
        let task = task_id(&state);
        insert_job(&state, &task, "settled", None);
        state
            .sqlite_writer
            .write(|c| repo::set_loop_state(c, &task, "cancelled", None, None))
            .unwrap();
        state
            .sqlite_writer
            .write(|c| repo::apply_terminal_event(c, "job", "settled", Some("run")))
            .unwrap();
        let state_name: String = state
            .sqlite_readers
            .read(|c| {
                c.query_row(
                    "SELECT loop_state FROM steward_tasks WHERE id=?1",
                    [&task],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(state_name, "cancelled");
    });
}

#[test]
fn ml_03_exact_trigger_queues_and_partial_does_not() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("run-partial", "テストを確認してね")).unwrap();
        super::on_user_message(&state, &turn("run-partial", "テストを確認してね"));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 0);
        prepare_runtime_run(&state, &turn("run-start", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-start", START_TRIGGER));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 1);
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_task_plans"), 1);
    });
}

#[test]
fn ml_03_memory_off_or_no_goal_is_noop() {
    with_memory(false, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("run-off", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-off", START_TRIGGER));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 0);
    });
    with_memory(true, || {
        let state = app_state(db());
        prepare_runtime_run(&state, &turn("run-nogoal", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-nogoal", START_TRIGGER));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 0);
    });
}

#[test]
fn ml_04_start_request_is_read_only_and_coding_shift_does_not_start() {
    assert!(!repo::request_forbidden(START_REQUEST));
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        state
            .situation
            .set_foreground_category_for_test(ForegroundCategory::Communication);
        super::inspect_coding_transition(&state, PRIMARY_CONVERSATION_ID).unwrap();
        state
            .situation
            .set_foreground_category_for_test(ForegroundCategory::Coding);
        super::inspect_coding_transition(&state, PRIMARY_CONVERSATION_ID).unwrap();
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 0);
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM conversation_messages WHERE role='user'"
            ),
            0
        );
    });
}

#[test]
fn ml_04_unrelated_turn_does_not_start() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("run-start", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-start", START_TRIGGER));
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM steward_tasks WHERE coding_job_id IS NULL"
            ),
            1
        );
        prepare_runtime_run(&state, &turn("run-other", "別の話")).unwrap();
        super::on_user_message(&state, &turn("run-other", "別の話"));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 1);
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM steward_tasks WHERE loop_state='queued' AND coding_job_id IS NULL"
            ),
            1
        );
    });
}

#[test]
fn ml_04_duplicate_dedupe_keeps_one_task() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("run-a", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-a", START_TRIGGER));
        prepare_runtime_run(&state, &turn("run-b", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-b", START_TRIGGER));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 1);
    });
}

#[test]
fn ml_05_hold_skips_insert_then_flush_one() {
    with_memory(true, || {
        let state = app_state(db());
        state
            .sqlite_writer
            .write(|connection| {
                let _ = workspace(connection);
                repo::register_with_options(
                    connection,
                    PRIMARY_CONVERSATION_ID,
                    "ws",
                    "tests pass",
                    "",
                    "test_report_obtained",
                    "read_test",
                    3,
                    60_000,
                    "silent",
                )
            })
            .unwrap();
        prepare_runtime_run(&state, &turn("run-hold", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-hold", START_TRIGGER));
        let id = task_id(&state);
        insert_job(&state, &id, "settled", None);
        state
            .situation
            .set_scene_attention_for_test("MEETING", "OBSERVE");
        assert!(speech_holds_tts(&state));
        state
            .sqlite_writer
            .write(|connection| super::report::publish(&state, connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM conversation_messages WHERE role='assistant'"
            ),
            0
        );
        state
            .situation
            .set_scene_attention_for_test("FOCUS", "OBSERVE");
        // Silent reports are normally aggregate-delayed. The report here was
        // already available before the meeting hold, so advance only its
        // durable availability to exercise hold release without TTS side
        // effects in this unit test.
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute("UPDATE steward_reports SET available_at_ms=0", [])
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .unwrap();
        super::flush_held_reports(&state, PRIMARY_CONVERSATION_ID).unwrap();
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM conversation_messages WHERE role='assistant'"
            ),
            1
        );
        let body: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT content FROM conversation_messages WHERE role='assistant'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        assert!(!body.to_ascii_lowercase().contains("confidence"));
    });
}

#[test]
fn dw_14_multiple_goals_keep_the_sibling_through_topic_switch_withdrawal_and_hold() {
    with_memory(true, || {
        let state = app_state(db());
        state
            .sqlite_writer
            .write(|connection| {
                let _ = workspace(connection);
                repo::register_with_options(
                    connection,
                    PRIMARY_CONVERSATION_ID,
                    "ws",
                    "A",
                    "A",
                    "test_report_obtained",
                    "read_test",
                    3,
                    60_000,
                    "silent",
                )
            })
            .unwrap();
        let second = state
            .sqlite_writer
            .write(|connection| {
                repo::register_with_options(
                    connection,
                    PRIMARY_CONVERSATION_ID,
                    "ws",
                    "B",
                    "B",
                    "test_report_obtained",
                    "read_test",
                    3,
                    60_000,
                    "silent",
                )
            })
            .unwrap();
        let second_goal = second["goalId"].as_str().unwrap().to_string();

        prepare_runtime_run(&state, &turn("dw-14-topic", "別の話題です")).unwrap();
        super::on_user_message(&state, &turn("dw-14-topic", "別の話題です"));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 0);

        prepare_runtime_run(&state, &turn("dw-14-source", START_TRIGGER)).unwrap();
        let source = state
            .sqlite_readers
            .read(|connection| repo::input_message_id(connection, "dw-14-source"))
            .unwrap()
            .unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                for work in repo::active_delegations(connection, PRIMARY_CONVERSATION_ID)? {
                    repo::queue_task(connection, &work, PRIMARY_CONVERSATION_ID, &source, "start")?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 2);

        let (first_goal, first_task, second_task): (String, String, String) = state
            .sqlite_readers
            .read(|connection| {
                let (first_goal, first_task) = connection
                    .query_row(
                        "SELECT d.goal_id,t.id FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id ORDER BY t.rowid LIMIT 1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(crate::database_error)?;
                let second_task = connection
                    .query_row(
                        "SELECT t.id FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id WHERE d.goal_id=?1",
                        [&second_goal],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)?;
                Ok((first_goal, first_task, second_task))
            })
            .unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                repo::withdraw_goal(connection, PRIMARY_CONVERSATION_ID, &first_goal)
            })
            .unwrap();
        let withdrawn_state: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT loop_state FROM steward_tasks WHERE id=?1",
                        [&first_task],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(withdrawn_state, "cancelled");
        let sibling_state: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT loop_state FROM steward_tasks WHERE id=?1",
                        [&second_task],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(sibling_state, "queued");

        insert_job(&state, &second_task, "settled", None);
        state
            .situation
            .set_scene_attention_for_test("MEETING", "OBSERVE");
        state
            .sqlite_writer
            .write(|connection| super::report::publish(&state, connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM conversation_messages WHERE role='assistant'",
            ),
            0
        );
        state
            .situation
            .set_scene_attention_for_test("FOCUS", "OBSERVE");
        // Silent reports use the normal aggregation delay. This scenario is
        // specifically about a report that was already ready but held by the
        // meeting, so make the durable outbox row available before releasing
        // the hold without starting a real TTS session in the test process.
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute("UPDATE steward_reports SET available_at_ms=0", [])
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .unwrap();
        super::flush_held_reports(&state, PRIMARY_CONVERSATION_ID).unwrap();
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM conversation_messages WHERE role='assistant'",
            ),
            1
        );
    });
}

#[test]
fn ml_06_withdraw_blocks_start_and_continue() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("run-w", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-w", START_TRIGGER));
        state
            .sqlite_writer
            .write(|connection| repo::withdraw(connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        prepare_runtime_run(&state, &turn("run-again", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-again", START_TRIGGER));
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM steward_tasks WHERE loop_state='queued'"
            ),
            0
        );
        prepare_runtime_run(&state, &turn("run-cont", CONTINUE_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-cont", CONTINUE_TRIGGER));
        let status: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT status FROM steward_goals ORDER BY rowid DESC LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(status, "withdrawn");
    });
}

#[test]
fn ml_06_withdraw_does_not_rewrite_done() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("run-done", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-done", START_TRIGGER));
        let id = task_id(&state);
        insert_job(&state, &id, "settled", None);
        state
            .sqlite_writer
            .write(|connection| super::report::publish(&state, connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                repo::withdraw(connection, PRIMARY_CONVERSATION_ID)?;
                repo::sync_from_coding(connection, PRIMARY_CONVERSATION_ID)
            })
            .unwrap();
        let state_name: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT loop_state FROM steward_tasks WHERE id=?1",
                        [&id],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(state_name, "done");
    });
}

#[test]
fn ml_07_reopen_maps_outcome_unknown_without_rerun() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saaa.sqlite3");
    {
        let connection = Connection::open(&path).unwrap();
        initialize_database(&connection).unwrap();
        let state = app_state(connection);
        with_memory(true, || {
            register_goal(&state);
            prepare_runtime_run(&state, &turn("run-r", START_TRIGGER)).unwrap();
            super::on_user_message(&state, &turn("run-r", START_TRIGGER));
            let task = task_id(&state);
            insert_job(&state, &task, "running", Some("launching"));
            state
                .sqlite_writer
                .write(|connection| {
                    connection
                        .execute(
                            "UPDATE coding_runs SET pid=NULL,state='running' WHERE id='run'",
                            [],
                        )
                        .map_err(crate::database_error)?;
                    Ok(())
                })
                .unwrap();
        });
    }
    let connection = Connection::open(&path).unwrap();
    initialize_database(&connection).unwrap();
    let job_state: String = connection
        .query_row("SELECT state FROM coding_jobs WHERE id='job'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(job_state, "outcome_unknown");
    let mapped = repo::map_coding_state(&job_state);
    assert_eq!(mapped, "awaiting_user");
    let starts: i64 = connection
        .query_row("SELECT COUNT(*) FROM coding_jobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(starts, 1);
    let state = app_state(connection);
    let listed = state
        .sqlite_writer
        .write(|connection| {
            repo::sync_from_coding(connection, PRIMARY_CONVERSATION_ID)?;
            repo::list(connection, PRIMARY_CONVERSATION_ID)
        })
        .unwrap();
    assert_eq!(listed[0]["loopState"], "awaiting_user");
    assert_eq!(count(&state, "SELECT COUNT(*) FROM coding_jobs"), 1);
}

#[test]
fn ml_08_acceptance_register_divert_complete_withdraw() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("run-chat", "別の話")).unwrap();
        super::on_user_message(&state, &turn("run-chat", "別の話"));
        prepare_runtime_run(&state, &turn("run-go", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-go", START_TRIGGER));
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_tasks"), 1);
        let id = task_id(&state);
        insert_job(&state, &id, "settled", None);
        state
            .sqlite_writer
            .write(|connection| super::report::publish(&state, connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM conversation_messages WHERE role='assistant'"
            ),
            1
        );
        let delivered = state
            .sqlite_readers
            .read(|connection| repo::list(connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        assert_eq!(delivered[0]["deliveryState"], "delivered");
        assert_eq!(delivered[0]["speechState"], "pending");
        state
            .sqlite_writer
            .write(|connection| repo::withdraw(connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        prepare_runtime_run(&state, &turn("run-late", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("run-late", START_TRIGGER));
        assert_eq!(
            count(
                &state,
                "SELECT COUNT(*) FROM steward_tasks WHERE loop_state='queued'"
            ),
            0
        );
        let listed = state
            .sqlite_readers
            .read(|connection| repo::list(connection, PRIMARY_CONVERSATION_ID))
            .unwrap();
        assert!(listed.as_array().is_some_and(|rows| !rows.is_empty()));
    });
}
