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
            crate::coding::repository::event(
                connection,
                "job",
                "run",
                job_state,
                serde_json::json!({}),
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
    assert_eq!(DATABASE_SCHEMA_VERSION, 38);
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
        include_str!("../schema.rs"),
        include_str!("../repository.rs"),
        include_str!("../reduce.rs"),
        include_str!("../report.rs"),
        include_str!("../commands.rs"),
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
            .write(|connection| {
                repo::apply_terminal_event(connection, "job", "settled", Some("run"))
            })
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
            .write(|connection| {
                repo::apply_terminal_event(connection, "job", "settled", Some("run"))
            })
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
