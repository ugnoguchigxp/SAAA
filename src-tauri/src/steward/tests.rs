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
    let _guard = env_lock().lock().expect("memory env lock");
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
    assert_eq!(DATABASE_SCHEMA_VERSION, 27);
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
    assert_eq!(version, 27);
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
fn ml_02_register_requires_workspace() {
    let state = app_state(db());
    let error = state.sqlite_writer.write(|connection| {
        repo::register(connection, PRIMARY_CONVERSATION_ID, "ws", "tests pass")
    });
    assert_eq!(error.unwrap_err(), "workspace_required");
}

#[test]
fn ml_02_second_active_goal_refused_and_no_delete() {
    let state = app_state(db());
    register_goal(&state);
    let second = state
        .sqlite_writer
        .write(|connection| repo::register(connection, PRIMARY_CONVERSATION_ID, "ws", "again"));
    assert!(second.is_err());
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
        register_goal(&state);
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
