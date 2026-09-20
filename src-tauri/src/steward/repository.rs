use super::{CONTINUE_TRIGGER, DEDUPE_SUFFIX, START_REQUEST, START_TRIGGER};
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(crate) struct ActiveWork {
    pub goal_id: String,
    pub goal_status: String,
    pub delegation_id: String,
    pub workspace_id: String,
    pub budget_runs: i64,
    pub budget_ms: i64,
    pub superseded: bool,
}

pub(crate) fn register(
    connection: &Connection,
    conversation_id: &str,
    workspace_id: &str,
    success_condition: &str,
) -> Result<Value, String> {
    if workspace_id.is_empty() || success_condition.trim().is_empty() {
        return Err("steward_register_invalid".into());
    }
    if !workspace_registered(connection, conversation_id, workspace_id)? {
        return Err("workspace_required".into());
    }
    let now = now_iso();
    let goal_id = new_id("goal");
    let delegation_id = new_id("delegation");
    connection
        .execute(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,superseded_by)
             VALUES(?1,?2,'user_explicit',?3,'active',?4,NULL)",
            params![goal_id, conversation_id, success_condition.trim(), now],
        )
        .map_err(|_| "active_goal_exists".to_string())?;
    connection
        .execute(
            "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at,superseded_by)
             VALUES(?1,?2,?3,?4,'read_test',3,60000,'both','active',?5,NULL)",
            params![delegation_id, goal_id, conversation_id, workspace_id, now],
        )
        .map_err(database_error)?;
    Ok(json!({"goalId":goal_id,"delegationId":delegation_id,"status":"active"}))
}

pub(crate) fn withdraw(connection: &Connection, conversation_id: &str) -> Result<Value, String> {
    let Some(work) = latest_work(connection, conversation_id)? else {
        return Ok(json!({"status":"none"}));
    };
    if work.goal_status == "withdrawn" || work.superseded {
        return Ok(json!({"status":"withdrawn","goalId":work.goal_id}));
    }
    let now = now_iso();
    let goal_id = new_id("goal");
    let delegation_id = new_id("delegation");
    let condition: String = connection
        .query_row(
            "SELECT success_condition FROM steward_goals WHERE id=?1",
            [&work.goal_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    connection
        .execute(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,superseded_by)
             VALUES(?1,?2,'user_explicit',?3,'withdrawn',?4,NULL)",
            params![goal_id, conversation_id, condition, now],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE steward_goals SET superseded_by=?1 WHERE id=?2",
            params![goal_id, work.goal_id],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at,superseded_by)
             VALUES(?1,?2,?3,?4,'read_test',3,60000,'both','withdrawn',?5,NULL)",
            params![delegation_id, goal_id, conversation_id, work.workspace_id, now],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE steward_delegations SET superseded_by=?1 WHERE id=?2",
            params![delegation_id, work.delegation_id],
        )
        .map_err(database_error)?;
    let jobs = active_job_ids(connection, conversation_id)?;
    connection
        .execute(
            "UPDATE steward_tasks SET loop_state='cancelled',updated_at=?1
             WHERE delegation_id=?2 AND loop_state IN ('queued','running','awaiting_user')",
            params![now, work.delegation_id],
        )
        .map_err(database_error)?;
    Ok(json!({"status":"withdrawn","goalId":goal_id,"jobs":jobs}))
}

pub(crate) fn list(connection: &Connection, conversation_id: &str) -> Result<Value, String> {
    let mut stmt = connection
        .prepare(
            "SELECT t.id,t.loop_state,t.dedupe_key,t.coding_job_id,g.status
             FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.conversation_id=?1
             ORDER BY t.rowid",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok(json!({
                "taskId": row.get::<_, String>(0)?,
                "loopState": row.get::<_, String>(1)?,
                "dedupeKey": row.get::<_, String>(2)?,
                "codingJobId": row.get::<_, Option<String>>(3)?,
                "goalStatus": row.get::<_, String>(4)?,
            }))
        })
        .map_err(database_error)?;
    Ok(json!(rows.filter_map(Result::ok).collect::<Vec<_>>()))
}

pub(crate) fn latest_work(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<ActiveWork>, String> {
    connection
        .query_row(
            "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,
                    CASE WHEN g.superseded_by IS NULL THEN 0 ELSE 1 END
             FROM steward_goals g
             JOIN steward_delegations d ON d.goal_id=g.id
             WHERE g.conversation_id=?1
             ORDER BY g.rowid DESC LIMIT 1",
            [conversation_id],
            |row| {
                Ok(ActiveWork {
                    goal_id: row.get(0)?,
                    goal_status: row.get(1)?,
                    delegation_id: row.get(2)?,
                    workspace_id: row.get(3)?,
                    budget_runs: row.get(4)?,
                    budget_ms: row.get(5)?,
                    superseded: row.get::<_, i64>(6)? != 0,
                })
            },
        )
        .optional()
        .map_err(database_error)
}

pub(crate) fn active_delegation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<ActiveWork>, String> {
    Ok(latest_work(connection, conversation_id)?
        .filter(|work| work.goal_status == "active" && !work.superseded))
}

pub(crate) fn queue_task(
    connection: &Connection,
    work: &ActiveWork,
    conversation_id: &str,
    source_id: &str,
    kind: &str,
) -> Result<Option<String>, String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role='user')",
            params![source_id, conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !exists {
        return Err("source_unavailable".into());
    }
    let key = format!("{}:{DEDUPE_SUFFIX}", work.workspace_id);
    let now = now_iso();
    let id = new_id("task");
    match connection.execute(
        "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,'queued',?7,?7)",
        params![id, work.delegation_id, conversation_id, kind, source_id, key, now],
    ) {
        Ok(_) => Ok(Some(id)),
        Err(error) if error.to_string().contains("UNIQUE") => Ok(None),
        Err(error) => Err(database_error(error)),
    }
}

pub(crate) fn set_loop_state(
    connection: &Connection,
    task_id: &str,
    state: &str,
    job_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE steward_tasks SET loop_state=?2,coding_job_id=COALESCE(?3,coding_job_id),last_error=?4,updated_at=?5 WHERE id=?1",
            params![task_id, state, job_id, error, now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn queued_task(
    connection: &Connection,
    delegation_id: &str,
) -> Result<Option<(String, String)>, String> {
    connection
        .query_row(
            "SELECT id,source_id FROM steward_tasks WHERE delegation_id=?1 AND loop_state='queued' ORDER BY rowid LIMIT 1",
            [delegation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)
}

pub(crate) fn inspectable_jobs(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT t.id,t.coding_job_id FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.conversation_id=?1 AND t.coding_job_id IS NOT NULL
               AND g.status='active' AND g.superseded_by IS NULL
               AND t.loop_state IN ('queued','running','awaiting_user')",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub(crate) fn active_job_ids(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<(String, u64)>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT t.coding_job_id,j.revision FROM steward_tasks t
             JOIN coding_jobs j ON j.id=t.coding_job_id
             WHERE t.conversation_id=?1 AND t.coding_job_id IS NOT NULL
               AND t.loop_state IN ('queued','running','awaiting_user')",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })
        .map_err(database_error)?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub(crate) fn budget_exceeded(connection: &Connection, work: &ActiveWork) -> Result<bool, String> {
    let runs: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM coding_runs r
             JOIN coding_jobs j ON j.id=r.job_id
             JOIN steward_tasks t ON t.coding_job_id=j.id
             JOIN steward_delegations d ON d.id=t.delegation_id
             WHERE d.workspace_id=?1 AND d.superseded_by IS NULL",
            [&work.workspace_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if runs >= work.budget_runs {
        return Ok(true);
    }
    let first: Option<String> = connection
        .query_row(
            "SELECT MIN(r.started_at) FROM coding_runs r
             JOIN coding_jobs j ON j.id=r.job_id
             JOIN steward_tasks t ON t.coding_job_id=j.id
             JOIN steward_delegations d ON d.id=t.delegation_id
             WHERE d.workspace_id=?1 AND d.superseded_by IS NULL",
            [&work.workspace_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let Some(start) = first else {
        return Ok(false);
    };
    match (start.parse::<u128>(), now_iso().parse::<u128>()) {
        (Ok(start), Ok(now)) => Ok(now.saturating_sub(start) > work.budget_ms as u128),
        _ => Ok(false),
    }
}

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
    let withdrawn = latest_work(connection, conversation_id)?
        .is_some_and(|work| work.goal_status == "withdrawn" || work.superseded);
    let mut stmt = connection
        .prepare(
            "SELECT t.id,t.loop_state,j.state FROM steward_tasks t
             LEFT JOIN coding_jobs j ON j.id=t.coding_job_id
             WHERE t.conversation_id=?1",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for (task_id, previous, job_state) in rows {
        let open = matches!(previous.as_str(), "queued" | "running" | "awaiting_user");
        let next = if withdrawn && open {
            "cancelled"
        } else if withdrawn {
            previous.as_str()
        } else {
            job_state
                .as_deref()
                .map(map_coding_state)
                .unwrap_or(previous.as_str())
        };
        if next != previous {
            set_loop_state(connection, &task_id, next, None, None)?;
        }
    }
    Ok(())
}

pub(crate) fn claim_terminals(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<String>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT id,loop_state FROM steward_tasks
             WHERE conversation_id=?1 AND report_json IS NULL
               AND loop_state IN ('done','failed','cancelled')",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut terminal = Vec::new();
    for (task_id, state) in rows {
        connection
            .execute(
                "UPDATE steward_tasks SET report_json=?2,updated_at=?3 WHERE id=?1",
                params![task_id, json!({"loopState":state}).to_string(), now_iso()],
            )
            .map_err(database_error)?;
        terminal.push(format!("task {task_id}: {state}"));
    }
    Ok(terminal)
}

pub(crate) fn enqueue_report(
    connection: &Connection,
    conversation_id: &str,
    digest: &str,
    held_reason: Option<&str>,
) -> Result<String, String> {
    let id = new_id("report");
    connection
        .execute(
            "INSERT INTO steward_reports(id,conversation_id,digest,held_reason,flushed,created_at)
             VALUES(?1,?2,?3,?4,0,?5)",
            params![id, conversation_id, digest, held_reason, now_iso()],
        )
        .map_err(database_error)?;
    Ok(id)
}

pub(crate) fn unflushed_digest(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT digest FROM steward_reports WHERE conversation_id=?1 AND flushed=0 ORDER BY rowid",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(database_error)?;
    let parts: Vec<String> = rows.filter_map(Result::ok).collect();
    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join("\n")))
    }
}

pub(crate) fn mark_flushed(connection: &Connection, conversation_id: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE steward_reports SET flushed=1 WHERE conversation_id=?1 AND flushed=0",
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
