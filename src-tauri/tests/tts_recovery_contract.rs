#[path = "../src/runtime/conversation_check/queue_recovery.rs"]
mod queue_recovery;

#[test]
fn interrupted_uncommitted_speech_is_not_regenerated_after_restart() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE task_queue_jobs (
           id TEXT, scope TEXT, kind TEXT, job_key TEXT, generation INTEGER,
           state TEXT, owner TEXT, lease_until_ms INTEGER, error TEXT, updated_at_ms INTEGER
         );
         CREATE TABLE conversation_messages (id TEXT, conversation_id TEXT, role TEXT);
         CREATE TABLE runtime_runs (id TEXT, status TEXT, error_message TEXT, completed_at TEXT);
         INSERT INTO task_queue_jobs VALUES
           ('q1','scope','ornith_result','uncertain',0,'queued',NULL,NULL,NULL,0),
           ('s1','scope','speech','uncertain',0,'interrupted',NULL,NULL,NULL,0),
           ('q2','scope','ornith_result','saved',0,'completed',NULL,NULL,NULL,0),
           ('s2','scope','speech','saved',0,'interrupted',NULL,NULL,NULL,0),
           ('q3','scope','ornith_result','failed_window',0,'queued',NULL,NULL,NULL,0),
           ('s3','scope','speech','failed_window',0,'failed',NULL,NULL,NULL,0);
         INSERT INTO conversation_messages VALUES ('reply_saved','scope','assistant');
         INSERT INTO runtime_runs VALUES
           ('run_uncertain','running',NULL,NULL),('run_saved','completed',NULL,NULL),
           ('run_failed_window','running',NULL,NULL);",
    )
    .unwrap();

    queue_recovery::mark_uncertain_speech(&db, "scope", "2026-09-27T00:00:00Z", 123).unwrap();

    let state = |id: &str| -> String {
        db.query_row(
            "SELECT state FROM task_queue_jobs WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(state("q1"), "failed");
    assert_eq!(state("s1"), "failed");
    assert_eq!(state("q2"), "completed");
    assert_eq!(state("s2"), "interrupted");
    assert_eq!(state("q3"), "failed");
    assert_eq!(state("s3"), "failed");
    let run: String = db
        .query_row(
            "SELECT status FROM runtime_runs WHERE id='run_uncertain'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(run, "failed");
    let recovered: String = db
        .query_row(
            "SELECT status FROM runtime_runs WHERE id='run_failed_window'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recovered, "failed");
}
