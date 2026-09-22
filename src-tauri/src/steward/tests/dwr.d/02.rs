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
#[test]
fn dw_r06_schedule_dispatch_shares_commit_path_and_gates() {
    let state = app_state(db());
    register(&state);
    let error = crate::steward::dispatch_scheduled(&state, "missing", "missing").unwrap_err();
    assert_eq!(error, "dispatch_gated");
    assert_eq!(count(&state, "SELECT COUNT(*) FROM coding_jobs"), 0);
}
#[test]
fn dw_r12_tests_pass_cannot_be_approved_without_evidence() {
    assert_eq!(
        crate::steward::commands::next_resolve_state("approve", "awaiting_user", "tests_pass")
            .unwrap_err(),
        "evidence_required"
    );
    assert_eq!(
        crate::steward::commands::next_resolve_state(
            "approve",
            "awaiting_user",
            "user_confirmation_required"
        )
        .unwrap(),
        "done"
    );
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
        .read(|c| {
            c.query_row(sql, [], |row| row.get(0))
                .map_err(crate::database_error)
        })
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
