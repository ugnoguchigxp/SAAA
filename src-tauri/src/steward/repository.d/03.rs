pub(crate) fn workspace_registered(
    connection: &Connection,
    conversation_id: &str,
    workspace_id: &str,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM coding_workspaces WHERE conversation_id=?1 AND id=?2)",
            params![conversation_id, workspace_id],
            |row| row.get(0),
        )
        .map_err(database_error)
}
pub(crate) fn input_message_id(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT input_message_id FROM runtime_runs WHERE id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}
pub(crate) fn last_foreground(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT last_foreground FROM steward_runtime WHERE conversation_id=?1",
            [conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}
pub(crate) fn store_foreground(
    connection: &Connection,
    conversation_id: &str,
    category: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO steward_runtime(conversation_id,last_foreground) VALUES(?1,?2)
             ON CONFLICT(conversation_id) DO UPDATE SET last_foreground=excluded.last_foreground",
            params![conversation_id, category],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(crate) fn map_coding_state(job_state: &str) -> &'static str {
    match job_state {
        "queued" => "queued",
        "starting" | "running" | "stopping" | "cancel_requested" => "running",
        "outcome_unknown" => "awaiting_user",
        "settled" => "done",
        "failed" => "failed",
        _ => "cancelled",
    }
}
pub(crate) fn sync_from_coding(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), String> {
    let mut stmt = connection
        .prepare(
            "SELECT t.id,t.loop_state,j.state,g.verifier,g.status FROM steward_tasks t
             LEFT JOIN coding_jobs j ON j.id=t.coding_job_id
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.conversation_id=?1",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for (task_id, previous, job_state, _verifier, goal_status) in rows {
        let withdrawn = goal_status == "withdrawn";
        let open = matches!(
            previous.as_str(),
            "queued"
                | "dispatching"
                | "running"
                | "awaiting_user"
                | "verifying"
                | "awaiting_dependency"
        );
        let next = if withdrawn && open {
            "cancelled"
        } else if withdrawn || matches!(previous.as_str(), "cancelled" | "outcome_unknown") {
            previous.as_str()
        } else {
            match job_state.as_deref() {
                Some("settled") => {
                    if matches!(previous.as_str(), "done" | "failed" | "cancelled") {
                        previous.as_str()
                    } else {
                        "verifying"
                    }
                }
                Some(state) => map_coding_state(state),
                None => previous.as_str(),
            }
        };
        if next != previous {
            set_loop_state(connection, &task_id, next, None, None)?;
        }
    }
    Ok(())
}
pub(crate) struct TerminalReport {
    pub(crate) task_id: String,
    pub(crate) task_revision: i64,
    pub(crate) goal_id: String,
    pub(crate) notify: String,
    pub(crate) digest: String,
}
pub(crate) fn claim_terminals(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<TerminalReport>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT t.id,t.revision,t.loop_state,g.id,d.notify FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.conversation_id=?1 AND t.report_json IS NULL
               AND t.loop_state IN ('done','failed','cancelled','awaiting_user','outcome_unknown')",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut terminal = Vec::new();
    for (task_id, task_revision, state, goal_id, notify) in rows {
        connection
            .execute(
                "UPDATE steward_tasks SET report_json=?2,updated_at=?3 WHERE id=?1",
                params![task_id, json!({"loopState":state}).to_string(), now_iso()],
            )
            .map_err(database_error)?;
        let mut report = TerminalReport {
            task_id: task_id.clone(),
            task_revision,
            goal_id,
            notify,
            digest: String::new(),
        };
        report.digest = super::report_content::compose(connection, &report, &state);
        terminal.push(report);
    }
    Ok(terminal)
}
pub(crate) fn enqueue_task_report(
    connection: &Connection,
    conversation_id: &str,
    terminal: &TerminalReport,
    held_reason: Option<&str>,
    available_at_ms: i64,
    speak_requested: bool,
) -> Result<String, String> {
    let id = new_id("report");
    connection.execute(
        "INSERT OR IGNORE INTO steward_reports(id,conversation_id,digest,held_reason,flushed,created_at,available_at_ms,task_id,task_revision,destination,delivery_state,speak_requested,speech_state)
         VALUES(?1,?2,?3,?4,0,?5,?6,?7,?8,'conversation','pending',?9,CASE WHEN ?9 THEN 'pending' ELSE 'not_requested' END)",
        params![id, conversation_id, terminal.digest, held_reason, now_iso(), available_at_ms, terminal.task_id, terminal.task_revision, speak_requested],
    ).map_err(database_error)?;
    Ok(id)
}
pub(crate) fn enqueue_report(
    connection: &Connection,
    conversation_id: &str,
    digest: &str,
    held_reason: Option<&str>,
    available_at_ms: i64,
) -> Result<String, String> {
    let id = new_id("report");
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM steward_reports WHERE conversation_id=?1 AND digest=?2)",
            params![conversation_id, digest],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if exists {
        return Ok(id);
    }
    connection
        .execute(
            "INSERT INTO steward_reports(id,conversation_id,digest,held_reason,flushed,created_at,available_at_ms)
             VALUES(?1,?2,?3,?4,0,?5,?6)",
            params![id, conversation_id, digest, held_reason, now_iso(), available_at_ms],
        )
        .map_err(database_error)?;
    Ok(id)
}
pub(crate) fn unflushed_digest(
    connection: &Connection,
    conversation_id: &str,
    now_ms: i64,
) -> Result<Option<String>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT digest FROM steward_reports WHERE conversation_id=?1 AND flushed=0 AND available_at_ms<=?2 ORDER BY rowid",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map(params![conversation_id, now_ms], |row| {
            row.get::<_, String>(0)
        })
        .map_err(database_error)?;
    let parts: Vec<String> = rows.filter_map(Result::ok).collect();
    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join("\n")))
    }
}
pub(crate) fn mark_flushed(
    connection: &Connection,
    conversation_id: &str,
    now_ms: i64,
    message_id: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE steward_reports SET flushed=1,delivery_state='delivered',message_id=?3 WHERE conversation_id=?1 AND flushed=0 AND available_at_ms<=?2",
            params![conversation_id, now_ms, message_id],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(crate) struct PendingSpeech {
    pub(crate) id: String,
    pub(crate) digest: String,
}
/// Claim only conversation-delivered reports. The claim is durable before any
/// TTS provider is asked to render or play, so a restart cannot replay sound.
pub(crate) fn claim_pending_speech(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> Result<Vec<PendingSpeech>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id,digest FROM steward_reports
             WHERE conversation_id=?1 AND flushed=1 AND speak_requested=1
               AND speech_state='pending' ORDER BY rowid",
        )
        .map_err(database_error)?;
    let reports = statement
        .query_map([conversation_id], |row| {
            Ok(PendingSpeech {
                id: row.get(0)?,
                digest: row.get(1)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    for report in &reports {
        connection
            .execute(
                "UPDATE steward_reports SET speech_state='starting',speech_run_id=?2
                 WHERE id=?1 AND speech_state='pending'",
                params![report.id, run_id],
            )
            .map_err(database_error)?;
    }
    Ok(reports)
}
pub(crate) fn mark_speech_state(
    connection: &Connection,
    run_id: &str,
    state: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE steward_reports SET speech_state=?2 WHERE speech_run_id=?1
             AND speech_state IN ('starting','playback_started')",
            params![run_id, state],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(crate) fn suppress_pending_speech(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE steward_reports SET speech_state='suppressed'
             WHERE conversation_id=?1 AND flushed=1 AND speak_requested=1
               AND speech_state='pending'",
            [conversation_id],
        )
        .map_err(database_error)?;
    Ok(())
}
pub(crate) fn start_request() -> &'static str {
    START_REQUEST
}
pub(crate) fn triggers() -> (&'static str, &'static str) {
    (START_TRIGGER, CONTINUE_TRIGGER)
}
pub(crate) fn request_forbidden(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    lower.contains("patch") || lower.contains("commit")
}
pub(crate) fn coding_enabled(state: &AppState) -> Result<bool, String> {
    state
        .sqlite_readers
        .read(crate::coding::repository::settings)
        .map(|settings| settings.enabled)
}
pub(crate) fn delegated_profile_available(connection: &Connection) -> Result<bool, String> {
    let settings = crate::coding::repository::settings(connection)?;
    Ok(settings.enabled
        && matches!(
            settings.profile.as_str(),
            "delegated-read-test-macos-v1" | "delegated-codex-sdk-macos-v1"
        ))
}
