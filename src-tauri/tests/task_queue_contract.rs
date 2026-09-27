fn database_error(error: rusqlite::Error) -> String {
    error.to_string()
}

#[path = "../src/task_queue.rs"]
mod task_queue;

#[path = "../src/runtime/conversation_check/queue_input_state.rs"]
mod queue_input_state;

#[test]
fn cancellation_uses_input_order_when_downstream_jobs_are_created_late() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    task_queue::migrate(&db).unwrap();
    db.execute_batch(
        "CREATE TABLE conversation_messages(id TEXT,conversation_id TEXT,role TEXT,content TEXT);
         CREATE TABLE runtime_runs(id TEXT,conversation_id TEXT,status TEXT,completed_at TEXT);
         INSERT INTO conversation_messages VALUES
           ('check_a','c','user','調査して'),('check_b','c','user','訂正して');
         INSERT INTO runtime_runs VALUES ('run_a','c','running',NULL),('run_b','c','running',NULL);",
    ).unwrap();
    task_queue::enqueue(&db, "c", "qwen", "user_input", "a", 0, "{}", None).unwrap();
    let input = task_queue::claim(&mut db, "qwen").unwrap().unwrap();
    task_queue::enqueue(&db, "c", "qwen", "user_input", "b", 0, "{}", None).unwrap();
    // A finishes routing only after B has arrived.
    task_queue::enqueue(&db, "c", "ornith", "ornith_task", "a", 0, "{}", None).unwrap();
    task_queue::finish(&db, &input).unwrap();
    let research = task_queue::claim(&mut db, "ornith").unwrap().unwrap();
    assert_eq!(
        queue_input_state::active_previous(&db, "c", "b").unwrap(),
        Some(("a".into(), "調査して".into()))
    );
    // Never treat a later input as the previous request.
    assert!(queue_input_state::active_previous(&db, "c", "a")
        .unwrap()
        .is_none());
    // A remains cancellable after reasoning completes, while its audio is queued.
    task_queue::finish(&db, &research).unwrap();
    task_queue::enqueue(&db, "c", "speech", "speech", "a", 0, "{}", None).unwrap();
    let speech = task_queue::claim(&mut db, "speech").unwrap().unwrap();
    assert_eq!(
        queue_input_state::active_previous(&db, "c", "b").unwrap(),
        Some(("a".into(), "調査して".into()))
    );
    let tx = db.transaction().unwrap();
    queue_input_state::cancel(&tx, "c", "a", "finished").unwrap();
    tx.commit().unwrap();
    assert!(task_queue::finish(&db, &speech).is_err());
    assert!(queue_input_state::active_previous(&db, "c", "b")
        .unwrap()
        .is_none());
    let status: (String, String) = db
        .query_row(
            "SELECT status,completed_at FROM runtime_runs WHERE id='run_a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, ("cancelled".into(), "finished".into()));
    assert_eq!(
        db.query_row(
            "SELECT status FROM runtime_runs WHERE id='run_b'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "running"
    );
}
