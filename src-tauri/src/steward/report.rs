use super::repository as repo;
use crate::situation::speech_holds_tts;
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, Connection};

pub(crate) fn publish(
    state: &AppState,
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), String> {
    super::driver::consume(connection)?;
    repo::sync_from_coding(connection, conversation_id)?;
    queue_terminals(
        state,
        connection,
        conversation_id,
        &repo::claim_terminals(connection, conversation_id)?,
    )
}

pub(crate) fn queue_terminals(
    state: &AppState,
    connection: &Connection,
    conversation_id: &str,
    terminals: &[repo::TerminalReport],
) -> Result<(), String> {
    let now = now_ms();
    for terminal in terminals {
        let (delivery, selection_mode, policy_revision, candidates) =
            notification_selection(connection, &terminal.goal_id, &terminal.notify, now)?;
        repo::enqueue_task_report(
            connection,
            conversation_id,
            terminal,
            speech_holds_tts(state).then_some("situation_hold"),
            if delivery == "silent" {
                now + 45_000
            } else {
                now
            },
            delivery != "silent",
        )?;
        let decision_id = format!(
            "ai-notification-{}",
            &crate::adaptive_improvement::digest(
                format!("{}:{}", terminal.goal_id, terminal.digest).as_bytes()
            )[..24]
        );
        let recorded: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM ai_decisions WHERE id=?1)",
                [&decision_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if !recorded {
            crate::adaptive_improvement::record_decision(
                connection,
                &crate::adaptive_improvement::DecisionObservation {
                    id: decision_id,
                    domain: crate::adaptive_improvement::Domain::Notification,
                    scope_key: terminal.goal_id.clone(),
                    event_seq: 0,
                    policy_revision,
                    candidate_fingerprint: crate::adaptive_improvement::fingerprint_for(
                        &candidates,
                    ),
                    eligible_candidates: candidates,
                    selected: delivery.to_string(),
                    selection_mode: selection_mode.to_string(),
                    source_refs_json:
                        serde_json::json!({"goalId": terminal.goal_id, "digest": terminal.digest})
                            .to_string(),
                },
                now,
            )?;
        }
    }
    if !speech_holds_tts(state) {
        flush_unflushed(connection, conversation_id, now)?;
    }
    Ok(())
}

pub(crate) fn flush_held_reports(state: &AppState, conversation_id: &str) -> Result<(), String> {
    if speech_holds_tts(state) {
        return Ok(());
    }
    state.sqlite_writer.write(|connection| {
        publish(state, connection, conversation_id)?;
        flush_unflushed(connection, conversation_id, now_ms())
    })?;
    // The worker is woken by durable report/event processing, not by a new
    // user turn. This also starts a dependency-satisfied successor.
    super::reduce::start_queued_for_conversation(state, conversation_id)?;
    start_pending_speech(state, conversation_id)
}

/// Delivery wake-up for the schedule loop.  It reads the outbox rather than a
/// foreground turn, so a completed background task does not require the user
/// to speak again.  Each conversation is still filtered through Situation
/// before a chat message is inserted.
pub(crate) fn flush_all_held_reports(state: &AppState) -> Result<(), String> {
    if speech_holds_tts(state) {
        return Ok(());
    }
    let conversations = state.sqlite_readers.read(|connection| {
        let mut statement = connection
            .prepare("SELECT DISTINCT conversation_id FROM steward_reports WHERE flushed=0")
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(database_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
    })?;
    for conversation_id in conversations {
        flush_held_reports(state, &conversation_id)?;
    }
    Ok(())
}

/// Starts speech strictly after the message-delivery transaction commits. The
/// report rows are claimed first; completion callbacks only move those rows
/// forward, and startup migration turns an interrupted claim into unknown.
fn start_pending_speech(state: &AppState, conversation_id: &str) -> Result<(), String> {
    if !crate::voice_behavior::upper_policies_allow_speech(state)? {
        return state
            .sqlite_writer
            .write(|connection| repo::suppress_pending_speech(connection, conversation_id));
    }
    let run_id = new_id("steward_speech");
    let reports = state
        .sqlite_writer
        .write(|connection| repo::claim_pending_speech(connection, conversation_id, &run_id))?;
    if reports.is_empty() {
        return Ok(());
    }
    let writer = state.sqlite_writer.clone();
    let callback_run_id = run_id.clone();
    let channel = tauri::ipc::Channel::new(move |body| {
        let state = speech_event_state(&body);
        if let Some(state) = state {
            let _ = writer
                .write(|connection| repo::mark_speech_state(connection, &callback_run_id, state));
        }
        Ok(())
    });
    if let Err(error) = tauri::async_runtime::block_on(
        state
            .streaming_tts
            .begin(state, &run_id, true, channel, None),
    ) {
        state
            .sqlite_writer
            .write(|connection| repo::mark_speech_state(connection, &run_id, "delivery_unknown"))?;
        return Err(error);
    }
    let digest = reports
        .iter()
        .map(|report| report.digest.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if state
        .streaming_tts
        .queue_utterance(&run_id, &digest)
        .is_err()
        || state.streaming_tts.finish(&run_id, &digest).is_err()
    {
        state.streaming_tts.cancel(&run_id);
        state
            .sqlite_writer
            .write(|connection| repo::mark_speech_state(connection, &run_id, "delivery_unknown"))?;
    }
    Ok(())
}

fn speech_event_state(body: &tauri::ipc::InvokeResponseBody) -> Option<&'static str> {
    let tauri::ipc::InvokeResponseBody::Json(json) = body else {
        return None;
    };
    if json.contains("\"type\":\"speechStarted\"") {
        Some("playback_started")
    } else if json.contains("\"type\":\"speechEnded\"") {
        Some("playback_finished")
    } else if json.contains("\"type\":\"speechFailed\"") {
        Some("delivery_unknown")
    } else {
        None
    }
}

fn flush_unflushed(
    connection: &Connection,
    conversation_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let Some(digest) = repo::unflushed_digest(connection, conversation_id, now_ms)? else {
        return Ok(());
    };
    let message_id = new_id("message");
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES(?1,?2,'assistant',?3,?4)",
            params![message_id, conversation_id, digest, now_iso()],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![now_iso(), conversation_id],
        )
        .map_err(database_error)?;
    record_delivered_notification_outcomes(connection, conversation_id, now_ms, &message_id)?;
    repo::mark_flushed(connection, conversation_id, now_ms, &message_id)
}

/// A notification becomes observable only after its outbox row is turned into a conversation
/// message.  Held and aggregate-delayed reports therefore intentionally have no outcome until
/// this point.  The durable source reference is the terminal digest, not the rendered aggregate.
fn record_delivered_notification_outcomes(
    connection: &Connection,
    conversation_id: &str,
    now_ms: i64,
    message_id: &str,
) -> Result<(), String> {
    let adaptive_schema_present: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ai_decisions')",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !adaptive_schema_present {
        return Ok(());
    }
    let mut reports = connection
        .prepare(
            "SELECT digest FROM steward_reports WHERE conversation_id=?1 AND flushed=0 AND available_at_ms<=?2",
        )
        .map_err(database_error)?;
    let digests = reports
        .query_map(params![conversation_id, now_ms], |row| {
            row.get::<_, String>(0)
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for digest in digests {
        let decision_ids = {
            let mut decisions = connection
                .prepare(
                    "SELECT id FROM ai_decisions WHERE domain='notification' AND json_extract(source_refs_json,'$.digest')=?1",
                )
                .map_err(database_error)?;
            let ids = decisions
                .query_map([&digest], |row| row.get::<_, String>(0))
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            ids
        };
        for decision_id in decision_ids {
            crate::adaptive_improvement::record_outcome(
                connection,
                &decision_id,
                Some(true),
                None,
                None,
                None,
                None,
                None,
                None,
                message_id,
                0,
                now_ms,
            )?;
        }
    }
    Ok(())
}

fn notification_selection(
    connection: &Connection,
    goal_id: &str,
    requested: &str,
    now: i64,
) -> Result<(String, &'static str, i64, Vec<String>), String> {
    let candidates = match requested {
        "silent" => vec!["silent".to_string()],
        "speak" => vec!["speak".to_string()],
        "both" => vec!["both".to_string(), "silent".to_string()],
        _ => return Err("steward_notification_invalid".into()),
    };
    let rules = candidates[0].clone();
    let settings = crate::persistence::load_role_routing_settings(connection)?;
    let (selected, mode, revision) =
        if settings.adaptive_improvement.enabled && settings.adaptive_improvement.notification {
            crate::adaptive_improvement::choose(
                connection,
                crate::adaptive_improvement::Domain::Notification,
                goal_id,
                &candidates,
                &rules,
                now,
            )?
        } else {
            (rules, "rules", 0)
        };
    Ok((selected, mode, revision, candidates))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
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
        assert!(repo::claim_pending_speech(
            &connection,
            crate::PRIMARY_CONVERSATION_ID,
            "later-run",
        )
        .expect("no replay")
        .is_empty());
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
}
