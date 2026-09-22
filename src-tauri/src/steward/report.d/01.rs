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
        super::outbox::flush_unflushed(connection, conversation_id, now)?;
    }
    Ok(())
}
pub(crate) fn flush_held_reports(state: &AppState, conversation_id: &str) -> Result<(), String> {
    if speech_holds_tts(state) {
        return Ok(());
    }
    let message_id = state.sqlite_writer.transact(|connection| {
        publish(state, connection, conversation_id)?;
        super::outbox::flush_unflushed(connection, conversation_id, now_ms())
    })?;
    if let Some(message_id) = message_id {
        state
            .steward_wake
            .emit_report(conversation_id, &message_id, 1, 0);
    }
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
    let Some(message_id) = super::outbox::flush_unflushed(connection, conversation_id, now_ms)?
    else {
        return Ok(());
    };
    record_delivered_notification_outcomes(connection, conversation_id, now_ms, &message_id)
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
