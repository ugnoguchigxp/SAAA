use super::*;
#[test]
fn aggregate_report_is_not_flushed_before_its_delivery_deadline() {
    let connection = rusqlite::Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let now = 1_000_i64;
    repo::enqueue_report(
        &connection,
        crate::PRIMARY_CONVERSATION_ID,
        "later",
        None,
        now + 45_000,
    )
    .expect("enqueue");
    assert_eq!(
        repo::unflushed_digest(&connection, crate::PRIMARY_CONVERSATION_ID, now).expect("read"),
        None
    );
    assert_eq!(
        repo::unflushed_digest(&connection, crate::PRIMARY_CONVERSATION_ID, now + 45_000)
            .expect("read"),
        Some("later".into())
    );
    repo::mark_flushed(&connection, crate::PRIMARY_CONVERSATION_ID, now, "message")
        .expect("premature mark is harmless");
    assert_eq!(
        repo::unflushed_digest(&connection, crate::PRIMARY_CONVERSATION_ID, now + 45_000)
            .expect("still pending"),
        Some("later".into())
    );
    repo::mark_flushed(
        &connection,
        crate::PRIMARY_CONVERSATION_ID,
        now + 45_000,
        "message",
    )
    .expect("due mark");
    assert_eq!(
        repo::unflushed_digest(&connection, crate::PRIMARY_CONVERSATION_ID, now + 45_000)
            .expect("flushed"),
        None
    );
}

#[test]
fn terminal_delivery_is_unique_per_task_revision_and_records_message_id() {
    let connection = rusqlite::Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let terminal = repo::TerminalReport {
        task_id: "task".into(),
        task_revision: 4,
        goal_id: "goal".into(),
        notify: "silent".into(),
        digest: "finished".into(),
    };
    repo::enqueue_task_report(
        &connection,
        crate::PRIMARY_CONVERSATION_ID,
        &terminal,
        None,
        0,
        false,
    )
    .expect("first");
    repo::enqueue_task_report(
        &connection,
        crate::PRIMARY_CONVERSATION_ID,
        &terminal,
        None,
        0,
        false,
    )
    .expect("replay");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM steward_reports", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 1);
    flush_unflushed(&connection, crate::PRIMARY_CONVERSATION_ID, 0).expect("flush");
    let state: (String, Option<String>) = connection
        .query_row(
            "SELECT delivery_state,message_id FROM steward_reports",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("delivery");
    assert_eq!(state.0, "delivered");
    assert!(state.1.is_some());
}

#[test]
fn speech_claim_is_not_replayed_after_a_restart() {
    let connection = rusqlite::Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let terminal = repo::TerminalReport {
        task_id: "task-speech".into(),
        task_revision: 1,
        goal_id: "goal".into(),
        notify: "speak".into(),
        digest: "finished".into(),
    };
    repo::enqueue_task_report(
        &connection,
        crate::PRIMARY_CONVERSATION_ID,
        &terminal,
        None,
        0,
        true,
    )
    .expect("enqueue speech");
    repo::mark_flushed(&connection, crate::PRIMARY_CONVERSATION_ID, 0, "message")
        .expect("message committed");
    let claimed =
        repo::claim_pending_speech(&connection, crate::PRIMARY_CONVERSATION_ID, "speech-run")
            .expect("claim");
    assert_eq!(claimed.len(), 1);
    super::super::schema::migrate(&connection).expect("restart migration");
    let state: String = connection
        .query_row("SELECT speech_state FROM steward_reports", [], |row| {
            row.get(0)
        })
        .expect("state");
    assert_eq!(state, "delivery_unknown");
    assert!(
        repo::claim_pending_speech(&connection, crate::PRIMARY_CONVERSATION_ID, "later-run",)
            .expect("no replay")
            .is_empty()
    );
}

#[test]
fn speech_callbacks_have_durable_terminal_mappings() {
    use tauri::ipc::InvokeResponseBody;
    for (event, state) in [
        ("speechStarted", Some("playback_started")),
        ("speechEnded", Some("playback_finished")),
        ("speechFailed", Some("delivery_unknown")),
        ("other", None),
    ] {
        let body = InvokeResponseBody::Json(format!(r#"{{"type":"{event}"}}"#));
        assert_eq!(super::speech_event_state(&body), state);
    }
}

/// This is deliberately opt-in: it uses the selected macOS System TTS and
/// real audio player. It proves the steward outbox path receives both
/// playback callbacks, rather than only unit-testing their JSON mapping.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires a macOS audio output device"]
fn steward_speech_delivery_reaches_playback_finished() {
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().expect("temporary TTS data directory");
    let connection = rusqlite::Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let mut state = crate::test_support::app_state(connection);
    state.data_directory = directory.path().to_owned();
    let terminal = repo::TerminalReport {
        task_id: "task-live-speech".into(),
        task_revision: 1,
        goal_id: "goal-live-speech".into(),
        notify: "speak".into(),
        digest: "音声報告の確認完了".into(),
    };
    state
        .sqlite_writer
        .write(|connection| {
            repo::enqueue_task_report(
                connection,
                crate::PRIMARY_CONVERSATION_ID,
                &terminal,
                None,
                0,
                true,
            )?;
            repo::mark_flushed(
                connection,
                crate::PRIMARY_CONVERSATION_ID,
                0,
                "message-live-speech",
            )
        })
        .expect("report is delivered before speech starts");
    super::start_pending_speech(&state, crate::PRIMARY_CONVERSATION_ID)
        .expect("speech session starts");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let speech_state: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row("SELECT speech_state FROM steward_reports", [], |row| {
                        row.get(0)
                    })
                    .map_err(crate::database_error)
            })
            .expect("speech state");
        if speech_state == "playback_finished" {
            break;
        }
        assert_ne!(speech_state, "delivery_unknown", "System TTS failed");
        assert!(Instant::now() < deadline, "speech state: {speech_state}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Exercises the same durable outbox callback on a real System TTS route
/// failure. A nonexistent macOS voice is rejected before playback, and the
/// report must become unknown rather than remaining claimed forever.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires macOS system TTS"]
fn steward_speech_delivery_records_a_real_tts_failure() {
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().expect("temporary TTS data directory");
    let connection = rusqlite::Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let mut state = crate::test_support::app_state(connection);
    state.data_directory = directory.path().to_owned();
    state
            .sqlite_writer
            .write(|connection| {
                let text: String = connection
                    .query_row(
                        "SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)?;
                let mut settings: serde_json::Value =
                    serde_json::from_str(&text).map_err(|_| "invalid provider fixture")?;
                settings["providers"][1]["voice"] =
                    serde_json::Value::String("SAAA voice that does not exist".into());
                connection
                    .execute(
                        "UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.model' AND key='default'",
                        [settings.to_string()],
                    )
                    .map_err(crate::database_error)?;
                let terminal = repo::TerminalReport {
                    task_id: "task-live-speech-failure".into(),
                    task_revision: 1,
                    goal_id: "goal-live-speech-failure".into(),
                    notify: "speak".into(),
                    digest: "この音声は失敗する".into(),
                };
                repo::enqueue_task_report(
                    connection,
                    crate::PRIMARY_CONVERSATION_ID,
                    &terminal,
                    None,
                    0,
                    true,
                )?;
                repo::mark_flushed(
                    connection,
                    crate::PRIMARY_CONVERSATION_ID,
                    0,
                    "message-live-speech-failure",
                )
            })
            .expect("report is delivered before speech starts");
    super::start_pending_speech(&state, crate::PRIMARY_CONVERSATION_ID)
        .expect("speech session starts before renderer failure");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let speech_state: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row("SELECT speech_state FROM steward_reports", [], |row| {
                        row.get(0)
                    })
                    .map_err(crate::database_error)
            })
            .expect("speech state");
        if speech_state == "delivery_unknown" {
            break;
        }
        assert!(Instant::now() < deadline, "speech state: {speech_state}");
        std::thread::sleep(Duration::from_millis(50));
    }
}
