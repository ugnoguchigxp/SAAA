use super::{
    contracts::{GoalProposal, MAX_ACTIVE_GOALS},
    CONTINUE_TRIGGER, DEDUPE_SUFFIX, START_REQUEST, START_TRIGGER,
};
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
    pub ops: String,
    pub verifier: String,
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
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,superseded_by,summary,revision,verifier)
             VALUES(?1,?2,'user_explicit',?3,'active',?4,NULL,'',1,'test_report_obtained')",
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

/// Accepts a model proposal only after the host has bound it to an existing
/// user message.  The proposal text itself is intentionally not trusted as an
/// authority grant.
pub(crate) fn propose(
    connection: &Connection,
    conversation_id: &str,
    proposal: &GoalProposal,
) -> Result<Value, String> {
    proposal.validate()?;
    let ops = proposal.operations_key();
    if ops == "invalid" {
        return Err("work_proposal_invalid".into());
    }
    let source_ok: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role IN ('user','transcript'))",
        params![proposal.source_message_id, conversation_id], |r| r.get(0),
    ).map_err(database_error)?;
    if !source_ok {
        return Err("source_unavailable".into());
    }
    if !workspace_registered(connection, conversation_id, &proposal.workspace_id)? {
        return Err("workspace_required".into());
    }
    let active: i64 = connection.query_row(
        "SELECT COUNT(*) FROM steward_goals WHERE conversation_id=?1 AND status='active' AND superseded_by IS NULL",
        [conversation_id], |r| r.get(0),
    ).map_err(database_error)?;
    if active as usize >= MAX_ACTIVE_GOALS {
        return Err("active_goal_limit".into());
    }
    let digest = format!(
        "{}:{}:{}",
        proposal.workspace_id,
        ops,
        proposal.verifier_key()
    );
    if let Some(existing_goal) = connection
        .query_row(
            "SELECT goal_id FROM steward_origin_bindings WHERE origin_kind='user_turn' AND origin_id=?1 AND operation_digest=?2",
            params![proposal.source_message_id, digest],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
    {
        let delegation_id: String = connection
            .query_row(
                "SELECT id FROM steward_delegations WHERE goal_id=?1 ORDER BY rowid DESC LIMIT 1",
                [&existing_goal],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        return Ok(json!({"goalId":existing_goal,"delegationId":delegation_id,"status":"active","duplicate":true}));
    }
    let now = now_iso();
    let goal_id = new_id("goal");
    let delegation_id = new_id("delegation");
    connection.execute(
        "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,superseded_by,summary,revision,verifier)
         VALUES(?1,?2,'user_explicit',?3,'active',?4,NULL,?5,1,?6)",
        params![goal_id, conversation_id, proposal.verifier_key(), now, proposal.summary.trim(), proposal.verifier_key()],
    ).map_err(database_error)?;
    connection.execute(
        "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at,superseded_by,revision,source_message_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'active',?9,NULL,1,?10)",
        params![delegation_id, goal_id, conversation_id, proposal.workspace_id, ops, proposal.budget_runs, proposal.budget_ms.min(i64::MAX as u64) as i64, proposal.notify_key(), now, proposal.source_message_id],
    ).map_err(database_error)?;
    connection.execute(
        "INSERT INTO steward_origin_bindings(id,goal_id,origin_kind,origin_id,operation_digest,created_at) VALUES(?1,?2,'user_turn',?3,?4,?5)",
        params![new_id("origin"), goal_id, proposal.source_message_id, digest, now],
    ).map_err(database_error)?;
    Ok(
        json!({"goalId":goal_id,"delegationId":delegation_id,"status":"active","requiresConfirmation":false}),
    )
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
    connection.execute(
        "UPDATE steward_budget_reservations SET state='released' WHERE task_id IN (SELECT id FROM steward_tasks WHERE delegation_id=?1) AND state='reserved'",
        [&work.delegation_id],
    ).map_err(database_error)?;
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
            "SELECT t.id,t.loop_state,t.dedupe_key,t.coding_job_id,g.status,g.id,g.summary,g.verifier,d.workspace_id,d.ops,d.budget_runs,d.budget_ms,d.notify
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
                "goalId": row.get::<_, String>(5)?,
                "summary": row.get::<_, String>(6)?,
                "verifier": row.get::<_, String>(7)?,
                "workspaceId": row.get::<_, String>(8)?,
                "operations": row.get::<_, String>(9)?,
                "budgetRuns": row.get::<_, i64>(10)?,
                "budgetMs": row.get::<_, i64>(11)?,
                "notify": row.get::<_, String>(12)?,
            }))
        })
        .map_err(database_error)?;
    Ok(json!(rows.filter_map(Result::ok).collect::<Vec<_>>()))
}

pub(crate) fn withdraw_goal(
    connection: &Connection,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Value, String> {
    let changed = connection.execute(
        "UPDATE steward_goals SET status='withdrawn',revision=revision+1 WHERE id=?1 AND conversation_id=?2 AND status='active' AND superseded_by IS NULL",
        params![goal_id, conversation_id],
    ).map_err(database_error)?;
    if changed == 0 {
        return Err("goal_unavailable".into());
    }
    connection.execute("UPDATE steward_delegations SET status='withdrawn',revision=revision+1 WHERE goal_id=?1", [goal_id]).map_err(database_error)?;
    connection.execute(
        "UPDATE steward_tasks SET loop_state='cancelled',updated_at=?1 WHERE delegation_id IN (SELECT id FROM steward_delegations WHERE goal_id=?2) AND loop_state IN ('queued','running','awaiting_user')",
        params![now_iso(), goal_id],
    ).map_err(database_error)?;
    connection.execute(
        "UPDATE steward_budget_reservations SET state='released' WHERE task_id IN (SELECT t.id FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id WHERE d.goal_id=?1) AND state='reserved'",
        [goal_id],
    ).map_err(database_error)?;
    Ok(json!({"status":"withdrawn","goalId":goal_id}))
}

/// Forgetting a user source revokes any work derived from it before the
/// conversation row disappears. Late coding events retain audit value but can
/// no longer advance a task or create a fresh report.
pub(crate) fn forget_source(connection: &Connection, source_id: &str) -> Result<(), String> {
    // Request process cancellation while the source/delegation authorization
    // is still present.  Once the task is marked cancelled (or the source row
    // is deleted), `coding::service::cancel` correctly refuses new control.
    let mut statement = connection
        .prepare(
            "SELECT j.id,j.revision,t.conversation_id FROM steward_tasks t
             JOIN coding_jobs j ON j.id=t.coding_job_id
             WHERE t.source_id=?1 AND t.loop_state IN ('running','awaiting_user')",
        )
        .map_err(database_error)?;
    let jobs = statement
        .query_map([source_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    for (job, revision, conversation) in jobs {
        crate::coding::service::cancel(
            connection,
            &conversation,
            &job,
            revision,
            "steward source forgotten",
        )?;
    }
    connection.execute(
        "UPDATE steward_tasks SET loop_state='cancelled',updated_at=?2 WHERE source_id=?1 AND loop_state IN ('queued','running','awaiting_user')",
        params![source_id, now_iso()],
    ).map_err(database_error)?;
    connection.execute(
        "UPDATE steward_budget_reservations SET state='released' WHERE task_id IN (SELECT id FROM steward_tasks WHERE source_id=?1) AND state='reserved'",
        [source_id],
    ).map_err(database_error)?;
    Ok(())
}

pub(crate) fn active_goal_job_ids(
    connection: &Connection,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Vec<(String, u64)>, String> {
    let mut stmt = connection.prepare(
        "SELECT j.id,j.revision FROM coding_jobs j JOIN steward_tasks t ON t.coding_job_id=j.id JOIN steward_delegations d ON d.id=t.delegation_id
         WHERE d.goal_id=?1 AND t.conversation_id=?2 AND t.loop_state IN ('queued','running','awaiting_user')",
    ).map_err(database_error)?;
    let rows = stmt
        .query_map(params![goal_id, conversation_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
}

pub(crate) fn latest_work(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<ActiveWork>, String> {
    connection
        .query_row(
            "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,
                    CASE WHEN g.superseded_by IS NULL THEN 0 ELSE 1 END,d.ops,g.verifier
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
                    ops: row.get(7)?,
                    verifier: row.get(8)?,
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

pub(crate) fn active_delegations(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ActiveWork>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier
         FROM steward_goals g JOIN steward_delegations d ON d.goal_id=g.id
         WHERE g.conversation_id=?1 AND g.status='active' AND g.superseded_by IS NULL
           AND d.status='active' AND d.superseded_by IS NULL ORDER BY g.rowid",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok(ActiveWork {
                goal_id: row.get(0)?,
                goal_status: row.get(1)?,
                delegation_id: row.get(2)?,
                workspace_id: row.get(3)?,
                budget_runs: row.get(4)?,
                budget_ms: row.get(5)?,
                superseded: row.get::<_, i64>(6)? != 0,
                ops: row.get(7)?,
                verifier: row.get(8)?,
            })
        })
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
}

pub(crate) fn active_goal_work(
    connection: &Connection,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Option<ActiveWork>, String> {
    connection
        .query_row(
            "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier
         FROM steward_goals g JOIN steward_delegations d ON d.goal_id=g.id
         WHERE g.id=?1 AND g.conversation_id=?2 AND g.status='active' AND g.superseded_by IS NULL
           AND d.status='active' AND d.superseded_by IS NULL",
            params![goal_id, conversation_id],
            |row| {
                Ok(ActiveWork {
                    goal_id: row.get(0)?,
                    goal_status: row.get(1)?,
                    delegation_id: row.get(2)?,
                    workspace_id: row.get(3)?,
                    budget_runs: row.get(4)?,
                    budget_ms: row.get(5)?,
                    superseded: row.get::<_, i64>(6)? != 0,
                    ops: row.get(7)?,
                    verifier: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(database_error)
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
            "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role IN ('user','transcript'))",
            params![source_id, conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !exists {
        return Err("source_unavailable".into());
    }
    // A delegation has at most one outstanding task.  Different user turns
    // may repeat the same request, but must not reserve budget or create a
    // second effect while the first task is still open.
    let key = format!("{}:{DEDUPE_SUFFIX}", work.delegation_id);
    let now = now_iso();
    let id = new_id("task");
    let reserved: i64 = connection.query_row(
        "SELECT COUNT(*) FROM steward_budget_reservations WHERE delegation_id=?1 AND state IN ('reserved','consumed')",
        [&work.delegation_id], |row| row.get(0),
    ).map_err(database_error)?;
    if reserved >= work.budget_runs {
        return Ok(None);
    }
    match connection.execute(
        "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,'queued',?7,?7)",
        params![id, work.delegation_id, conversation_id, kind, source_id, key, now],
    ) {
        Ok(_) => {
            connection.execute(
                "INSERT INTO steward_budget_reservations(id,delegation_id,task_id,state,created_at) VALUES(?1,?2,?3,'reserved',?4)",
                params![new_id("budget"), work.delegation_id, id, now],
            ).map_err(database_error)?;
            connection.execute(
                "INSERT INTO steward_dispatch_intents(id,task_id,state,idempotency_key,created_at,updated_at) VALUES(?1,?2,'pending',?3,?4,?4)",
                params![new_id("intent"), id, format!("{}:{}", work.delegation_id, source_id), now],
            ).map_err(database_error)?;
            Ok(Some(id))
        }
        Err(error) if error.to_string().contains("UNIQUE") => Ok(None),
        Err(error) => Err(database_error(error)),
    }
}

pub(crate) fn claim_dispatch(connection: &Connection, task_id: &str) -> Result<bool, String> {
    Ok(connection.execute(
        "UPDATE steward_dispatch_intents SET state='dispatching',updated_at=?2 WHERE task_id=?1 AND state='pending'",
        params![task_id, now_iso()],
    ).map_err(database_error)? == 1)
}

pub(crate) fn settle_dispatch(
    connection: &Connection,
    task_id: &str,
    receipt: Option<&Value>,
    failed: bool,
) -> Result<(), String> {
    connection.execute(
        "UPDATE steward_dispatch_intents SET state=?2,receipt_json=?3,updated_at=?4 WHERE task_id=?1 AND state='dispatching'",
        params![task_id, if failed { "failed" } else { "accepted" }, receipt.map(Value::to_string), now_iso()],
    ).map_err(database_error)?;
    Ok(())
}

/// Applies an already-committed coding terminal event.  `settled` only marks a
/// verifier as satisfied for report-obtained work; it never claims tests pass.
pub(crate) fn apply_terminal_event(
    connection: &Connection,
    job_id: &str,
    kind: &str,
) -> Result<(), String> {
    let Some((task_id, _conversation_id, verifier, goal_status, task_state)): Option<(String, String, String, String, String)> = connection.query_row(
        "SELECT t.id,t.conversation_id,g.verifier,g.status,t.loop_state FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id JOIN steward_goals g ON g.id=d.goal_id WHERE t.coding_job_id=?1",
        [job_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).optional().map_err(database_error)? else { return Ok(()); };
    if goal_status != "active"
        || !matches!(task_state.as_str(), "queued" | "running" | "awaiting_user")
    {
        return Ok(());
    }
    let state = match kind {
        "failed" | "interrupted" => "failed",
        "settled" if verifier == "test_report_obtained" => "done",
        "settled" => "awaiting_user",
        _ => return Ok(()),
    };
    set_loop_state(connection, &task_id, state, None, None)?;
    let plan_decision_id = format!("ai-plan-{task_id}");
    let adaptive_schema_ready: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ai_decisions')",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let has_plan_decision: bool = adaptive_schema_ready
        && connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM ai_decisions WHERE id=?1)",
                [&plan_decision_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
    if has_plan_decision {
        crate::adaptive_improvement::record_outcome(
            connection,
            &plan_decision_id,
            Some(kind == "settled"),
            (state == "done").then_some(true),
            None,
            None,
            None,
            None,
            None,
            job_id,
            1,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0),
        )?;
    }
    // `claim_terminals` owns the outbox insertion. It needs the delegation's notification
    // policy, so terminal consumption must not manufacture an immediate report here.
    Ok(())
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
            "UPDATE steward_tasks SET loop_state=?2,coding_job_id=COALESCE(?3,coding_job_id),last_error=?4,updated_at=?5,revision=revision+1 WHERE id=?1 AND loop_state!=?2",
            params![task_id, state, job_id, error, now_iso()],
        )
        .map_err(database_error)?;
    match state {
        "running" => {
            connection.execute("UPDATE steward_budget_reservations SET state='consumed' WHERE task_id=?1 AND state='reserved'", [task_id]).map_err(database_error)?;
        }
        "cancelled" => {
            connection.execute("UPDATE steward_budget_reservations SET state='released' WHERE task_id=?1 AND state='reserved'", [task_id]).map_err(database_error)?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn persist_task_plan(
    connection: &Connection,
    task_id: &str,
    recipe: &str,
    request: &str,
    selection_mode: &str,
    policy_revision: i64,
) -> Result<(), String> {
    connection.execute(
        "INSERT OR IGNORE INTO steward_task_plans(id,task_id,revision,recipe,request,selection_mode,policy_revision,created_at)
         VALUES(?1,?2,1,?3,?4,?5,?6,?7)",
        params![new_id("plan"), task_id, recipe, request, selection_mode, policy_revision, now_iso()],
    ).map_err(database_error)?;
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

pub(crate) fn next_queued_work(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<(ActiveWork, String)>, String> {
    connection.query_row(
        "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier,t.id
         FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id JOIN steward_goals g ON g.id=d.goal_id
         WHERE t.conversation_id=?1 AND t.loop_state='queued' AND g.status='active' AND g.superseded_by IS NULL
           AND d.status='active' AND d.superseded_by IS NULL ORDER BY t.rowid LIMIT 1",
        [conversation_id], |r| Ok((ActiveWork { goal_id:r.get(0)?, goal_status:r.get(1)?, delegation_id:r.get(2)?, workspace_id:r.get(3)?, budget_runs:r.get(4)?, budget_ms:r.get(5)?, superseded:r.get::<_, i64>(6)? != 0, ops:r.get(7)?, verifier:r.get(8)? }, r.get(9)?)),
    ).optional().map_err(database_error)
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
             WHERE d.id=?1 AND d.superseded_by IS NULL",
            [&work.delegation_id],
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
             WHERE d.id=?1 AND d.superseded_by IS NULL",
            [&work.delegation_id],
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
            "SELECT t.id,t.loop_state,j.state,g.verifier FROM steward_tasks t
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
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for (task_id, previous, job_state, verifier) in rows {
        let open = matches!(previous.as_str(), "queued" | "running" | "awaiting_user");
        let next = if withdrawn && open {
            "cancelled"
        } else if withdrawn {
            previous.as_str()
        } else {
            match job_state.as_deref() {
                Some("settled") if verifier != "test_report_obtained" => "awaiting_user",
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
               AND t.loop_state IN ('done','failed','cancelled')",
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
        terminal.push(TerminalReport {
            task_id: task_id.clone(),
            task_revision,
            goal_id,
            notify,
            digest: format!("task {task_id}: {state}"),
        });
    }
    Ok(terminal)
}

pub(crate) fn enqueue_task_report(
    connection: &Connection,
    conversation_id: &str,
    terminal: &TerminalReport,
    held_reason: Option<&str>,
    available_at_ms: i64,
) -> Result<String, String> {
    let id = new_id("report");
    connection.execute(
        "INSERT OR IGNORE INTO steward_reports(id,conversation_id,digest,held_reason,flushed,created_at,available_at_ms,task_id,task_revision,destination,delivery_state)
         VALUES(?1,?2,?3,?4,0,?5,?6,?7,?8,'conversation','pending')",
        params![id, conversation_id, terminal.digest, held_reason, now_iso(), available_at_ms, terminal.task_id, terminal.task_revision],
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

pub(crate) fn delegated_profile_available(state: &AppState) -> Result<bool, String> {
    state
        .sqlite_readers
        .read(crate::coding::repository::settings)
        .map(|settings| settings.enabled && settings.profile == "delegated-read-test-macos-v1")
}
