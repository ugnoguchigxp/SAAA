#[path = "../../../src-tauri/src/conversation_host/repository.rs"]
#[allow(dead_code)]
mod repository;

use repository::{accept_input, adopt_control, checkpoint_clause, complete_response, open_session};
use rusqlite::Connection;
use saaa_conversation_core::contracts::{Action, TextInput};

fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE conversations (
           id TEXT PRIMARY KEY,title TEXT,task_mode TEXT NOT NULL,
           created_at TEXT NOT NULL,updated_at TEXT NOT NULL
         );
         CREATE TABLE conversation_messages (
           id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id),
           role TEXT NOT NULL,content TEXT NOT NULL,created_at TEXT NOT NULL
         );",
    )
    .unwrap();
    repository::migrate(&db).unwrap();
    db
}

fn ready_database() -> Connection {
    let mut db = database();
    open_session(
        &mut db,
        "session-1",
        "conversation-1",
        "fingerprint",
        "key-1",
        "now",
    )
    .unwrap();
    assert!(repository::set_resource_state(
        &mut db,
        "session-1",
        "preparing",
        "ready",
        Some("connection-1"),
        "now"
    )
    .unwrap());
    db
}

fn input(id: &str, text: &str) -> TextInput {
    TextInput {
        session_id: "session-1".into(),
        input_id: id.into(),
        text: text.into(),
    }
}

#[test]
fn receipt_is_atomic_and_idempotent_by_original_bytes() {
    let mut db = ready_database();
    let first = accept_input(&mut db, &input("input-1", "日本語"), "now").unwrap();
    assert!(!first.duplicate);
    let duplicate = accept_input(&mut db, &input("input-1", "日本語"), "later").unwrap();
    assert_eq!(duplicate.response_id, first.response_id);
    assert!(duplicate.duplicate);
    assert_eq!(
        accept_input(&mut db, &input("input-1", "日本語 "), "later"),
        Err(repository::AcceptError::Conflict)
    );
    assert_eq!(
        accept_input(&mut db, &input("input-2", "別の入力"), "now"),
        Err(repository::AcceptError::Busy)
    );
    let user_count: i64 = db
        .query_row(
            "SELECT count(*) FROM conversation_messages WHERE role='user'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(user_count, 1);
}

#[test]
fn public_body_requires_control_and_completion_commit() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "挨拶して"), "now").unwrap();
    assert!(!checkpoint_clause(&mut db, &receipt.response_id, "こんにちは。", 0, "now").unwrap());
    assert!(adopt_control(&mut db, &receipt.response_id, Action::Reply, "now").unwrap());
    assert!(checkpoint_clause(&mut db, &receipt.response_id, "こんにちは。", 0, "now").unwrap());
    assert!(!checkpoint_clause(&mut db, &receipt.response_id, "重複。", 0, "now").unwrap());
    assert!(complete_response(&mut db, &receipt.response_id, "よろしく。", "now").unwrap());
    assert!(!complete_response(&mut db, &receipt.response_id, "重複", "now").unwrap());
    let (state, body): (String, String) = db
        .query_row(
            "SELECT frontdesk_state,public_text FROM conversation_preview_responses WHERE response_id=?1",
            [&receipt.response_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "completed");
    assert_eq!(body, "こんにちは。よろしく。");
    let assistant: String = db
        .query_row(
            "SELECT content FROM conversation_messages WHERE role='assistant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(assistant, body);
}

#[test]
fn delegate_persists_only_fixed_explanation() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "仕事を任せる"), "now").unwrap();
    assert!(adopt_control(&mut db, &receipt.response_id, Action::Delegate, "now").unwrap());
    assert!(!checkpoint_clause(&mut db, &receipt.response_id, "受け付けました", 0, "now").unwrap());
    let body: String = db
        .query_row(
            "SELECT content FROM conversation_messages WHERE role='assistant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        body,
        saaa_conversation_core::frontdesk::DELEGATE_EXPLANATION
    );
}

#[test]
fn failed_delegate_drain_records_request_failure_without_model_body() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "仕事を任せる"), "now").unwrap();
    assert!(adopt_control(&mut db, &receipt.response_id, Action::Delegate, "now").unwrap());
    assert!(repository::fail_response(
        &mut db,
        &receipt.response_id,
        "qwen",
        "stream_interrupted",
        "later"
    )
    .unwrap());
    let (state, body, code): (String, String, String) = db.query_row(
        "SELECT frontdesk_state,public_text,failure_code FROM conversation_preview_responses WHERE response_id=?1",
        [&receipt.response_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))
    ).unwrap();
    assert_eq!(state, "unsupported");
    assert_eq!(
        body,
        saaa_conversation_core::frontdesk::DELEGATE_EXPLANATION
    );
    assert_eq!(code, "stream_interrupted");
}

#[test]
fn speech_generation_rejects_stale_completion() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "読む"), "now").unwrap();
    assert!(repository::update_speech(
        &mut db,
        &receipt.response_id,
        0,
        "idle",
        "collecting",
        None,
        "now"
    )
    .unwrap());
    assert!(!repository::update_speech(
        &mut db,
        &receipt.response_id,
        1,
        "collecting",
        "played",
        None,
        "now"
    )
    .unwrap());
}

#[test]
fn stop_invalidates_old_playback_without_claiming_it_stopped() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "読む"), "now").unwrap();
    assert!(repository::update_speech(
        &mut db,
        &receipt.response_id,
        0,
        "idle",
        "collecting",
        None,
        "now"
    )
    .unwrap());
    assert_eq!(
        repository::request_speech_stop(&mut db, &receipt.response_id, "later").unwrap(),
        Some(1)
    );
    assert!(!repository::update_speech(
        &mut db,
        &receipt.response_id,
        0,
        "collecting",
        "played",
        None,
        "late"
    )
    .unwrap());
    assert!(
        !repository::fail_speech(&mut db, &receipt.response_id, "stale_playback", "late").unwrap()
    );
    assert!(repository::update_speech(
        &mut db,
        &receipt.response_id,
        1,
        "stopping",
        "stopped",
        None,
        "confirmed"
    )
    .unwrap());
}

#[test]
fn cancellation_wins_against_late_completion() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "読む"), "now").unwrap();
    assert!(adopt_control(&mut db, &receipt.response_id, Action::Reply, "now").unwrap());
    assert!(repository::cancel_response(&mut db, &receipt.response_id, "stop").unwrap());
    assert!(!complete_response(&mut db, &receipt.response_id, "遅着", "late").unwrap());
    let assistant_count: i64 = db
        .query_row(
            "SELECT count(*) FROM conversation_messages WHERE role='assistant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(assistant_count, 0);
}

#[test]
fn completed_answer_survives_late_response_cancel() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "読む"), "now").unwrap();
    assert!(adopt_control(&mut db, &receipt.response_id, Action::Reply, "now").unwrap());
    assert!(complete_response(&mut db, &receipt.response_id, "完了。", "done").unwrap());
    assert!(!repository::cancel_response(&mut db, &receipt.response_id, "late").unwrap());
    assert_eq!(
        repository::request_speech_stop(&mut db, &receipt.response_id, "late").unwrap(),
        Some(1)
    );
    let state: String = db
        .query_row(
            "SELECT frontdesk_state FROM conversation_preview_responses WHERE response_id=?1",
            [&receipt.response_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "completed");
}

#[test]
fn cancelled_delegate_does_not_leave_an_orphan_answer() {
    let mut db = ready_database();
    let receipt = accept_input(&mut db, &input("input-1", "任せて"), "now").unwrap();
    assert!(repository::cancel_response(&mut db, &receipt.response_id, "stop").unwrap());
    assert!(!adopt_control(&mut db, &receipt.response_id, Action::Delegate, "late").unwrap());
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM conversation_messages WHERE role='assistant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn closed_resource_cannot_return_to_ready_or_failed() {
    let mut db = ready_database();
    assert!(
        repository::set_resource_state(&mut db, "session-1", "ready", "closing", None, "now")
            .unwrap()
    );
    assert!(
        repository::set_resource_state(&mut db, "session-1", "closing", "closed", None, "now")
            .unwrap()
    );
    assert!(!repository::set_resource_state(
        &mut db,
        "session-1",
        "closed",
        "failed",
        None,
        "late"
    )
    .unwrap());
    assert!(
        !repository::set_resource_state(&mut db, "session-1", "closed", "ready", None, "late")
            .unwrap()
    );
    let state: String = db
        .query_row(
            "SELECT resource_state FROM conversation_preview_sessions WHERE session_id='session-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "closed");
}
