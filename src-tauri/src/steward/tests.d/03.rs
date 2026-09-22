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
#[test]
fn ui_queue_reorder_is_durable_and_changes_dispatch_priority() {
    let connection = db();
    connection
        .execute_batch(&format!(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,summary) VALUES
               ('g1','{PRIMARY_CONVERSATION_ID}','user_explicit','one','active','1','one'),
               ('g2','{PRIMARY_CONVERSATION_ID}','user_explicit','two','active','2','two');
             INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at) VALUES
               ('d1','g1','{PRIMARY_CONVERSATION_ID}','ws','read',2,1000,'silent','active','1'),
               ('d2','g2','{PRIMARY_CONVERSATION_ID}','ws','read',2,1000,'silent','active','2');
             INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at,queue_rank) VALUES
               ('t1','d1','{PRIMARY_CONVERSATION_ID}','start','s1','k1','queued','1','1',1),
               ('t2','d2','{PRIMARY_CONVERSATION_ID}','start','s2','k2','queued','2','2',2);"
        ))
        .unwrap();

    repo::reorder_queue(
        &connection,
        PRIMARY_CONVERSATION_ID,
        &["t2".into(), "t1".into()],
    )
    .unwrap();

    let ordered = connection
        .prepare("SELECT id FROM steward_tasks ORDER BY queue_rank")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ordered, vec!["t2", "t1"]);
    assert_eq!(
        repo::next_queued_work(&connection, PRIMARY_CONVERSATION_ID)
            .unwrap()
            .map(|(_, task)| task),
        Some("t2".into())
    );
}
