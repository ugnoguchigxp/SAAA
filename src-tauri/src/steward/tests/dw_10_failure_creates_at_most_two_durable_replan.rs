use super::*;
#[test]
pub(super) fn dw_10_failure_creates_at_most_two_durable_replans() {
    with_memory(true, || {
        let state = app_state(db());
        register_goal(&state);
        prepare_runtime_run(&state, &turn("dw-10-replan", START_TRIGGER)).unwrap();
        super::on_user_message(&state, &turn("dw-10-replan", START_TRIGGER));
        let first = task_id(&state);
        insert_job(&state, &first, "running", None);
        state
            .sqlite_writer
            .write(|connection| {
                repo::apply_terminal_event(connection, "job", "failed", Some("run"))
            })
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
                    .execute("DELETE FROM coding_events", [])
                    .map_err(crate::database_error)?;
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
            .write(|connection| {
                repo::apply_terminal_event(connection, "job", "failed", Some("run"))
            })
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
                    .execute("DELETE FROM coding_events", [])
                    .map_err(crate::database_error)?;
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
            .write(|connection| {
                repo::apply_terminal_event(connection, "job", "failed", Some("run"))
            })
            .unwrap();
        assert_eq!(count(&state, "SELECT COUNT(*) FROM steward_goal_plans"), 3);
    });
}
#[test]
pub(super) fn dw_03_proposal_binds_only_a_persisted_user_source_and_allows_multiple_goals() {
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
pub(super) fn dw_02_reservation_is_durable_and_cancelled_before_dispatch_is_released() {
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
pub(super) fn dw_06_dispatch_intent_is_claimed_once_and_records_receipt() {
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
pub(super) fn dw_06_restart_marks_unreceived_dispatch_unknown_without_reclaiming() {
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
            super::super::schema::migrate(c).map_err(crate::database_error)
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
pub(super) fn dw_13_forgetting_source_cancels_derived_task() {
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
pub(super) fn dw_13_forgetting_source_requests_stop_for_running_coding_job() {
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
pub(super) fn dw_13_late_terminal_event_cannot_revive_cancelled_task() {
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
pub(super) fn ml_03_exact_trigger_queues_and_partial_does_not() {
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
pub(super) fn ml_03_memory_off_or_no_goal_is_noop() {
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
pub(super) fn ml_04_start_request_is_read_only_and_coding_shift_does_not_start() {
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
pub(super) fn ml_04_unrelated_turn_does_not_start() {
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
pub(super) fn ml_04_duplicate_dedupe_keeps_one_task() {
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
