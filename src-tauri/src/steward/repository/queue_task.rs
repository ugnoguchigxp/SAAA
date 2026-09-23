use super::*;
pub(crate) fn queue_task(
    connection: &Connection,
    work: &ActiveWork,
    conversation_id: &str,
    source_id: &str,
    kind: &str,
) -> Result<Option<String>, String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role IN ('user','transcript'))
             OR EXISTS(SELECT 1 FROM steward_source_bindings WHERE source_id=?1 AND source_kind='ui_receipt')",
            params![source_id, conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !exists {
        return Err("source_unavailable".into());
    }
    let Some((goal_plan_id, plan_step_id)) = next_ready_plan_step(connection, work)? else {
        return Ok(None);
    };
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
        "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at,goal_plan_id,plan_step_id)
         VALUES(?1,?2,?3,?4,?5,?6,'queued',?7,?7,?8,?9)",
        params![id, work.delegation_id, conversation_id, kind, source_id, key, now, goal_plan_id, plan_step_id],
    ) {
        Ok(_) => {
            connection.execute(
                "UPDATE steward_tasks SET queue_rank=(SELECT COALESCE(MAX(queue_rank),0)+1 FROM steward_tasks WHERE conversation_id=?2 AND id<>?1) WHERE id=?1",
                params![id, conversation_id],
            ).map_err(database_error)?;
            connection.execute(
                "INSERT INTO steward_budget_reservations(id,delegation_id,task_id,state,created_at) VALUES(?1,?2,?3,'reserved',?4)",
                params![new_id("budget"), work.delegation_id, id, now],
            ).map_err(database_error)?;
            connection.execute(
                "INSERT INTO steward_dispatch_intents(id,task_id,state,idempotency_key,created_at,updated_at) VALUES(?1,?2,'pending',?3,?4,?4)",
                params![new_id("intent"), id, format!("{}:{}:{}", work.delegation_id, source_id, id), now],
            ).map_err(database_error)?;
            let recipe = match plan_step_id.as_str() {
                "read" => "read",
                "test" | "test_run" => "test_run",
                _ => work.ops.as_str(),
            };
            persist_task_plan(
                connection,
                &id,
                recipe,
                match recipe {
                    "read" => "Inspect the existing failure evidence in this workspace and report causes. Do not run tests or change files.",
                    "test_run" => "Run the relevant existing tests in this workspace and report their result. Do not change files.",
                    _ => START_REQUEST,
                },
                "host_selected",
                1,
            )?;
            Ok(Some(id))
        }
        Err(error) if error.to_string().contains("UNIQUE") => Ok(None),
        Err(error) => Err(database_error(error)),
    }
}
pub(super) fn next_ready_plan_step(
    connection: &Connection,
    work: &ActiveWork,
) -> Result<Option<(String, String)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT p.id,s.step_id,s.depends_on_json FROM steward_goal_plans p
             JOIN steward_plan_steps s ON s.plan_id=p.id
             WHERE p.goal_id=?1 AND p.revision=(SELECT MAX(revision) FROM steward_goal_plans WHERE goal_id=?1)
             ORDER BY s.ordinal",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([&work.goal_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        let (plan_id, step_id, dependencies) = row.map_err(database_error)?;
        let already_created: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM steward_tasks WHERE delegation_id=?1 AND goal_plan_id=?2 AND plan_step_id=?3)",
                params![work.delegation_id, plan_id, step_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if already_created {
            continue;
        }
        let dependencies: Vec<String> =
            serde_json::from_str(&dependencies).map_err(|_| "steward_plan_invalid")?;
        let mut ready = true;
        for dependency in dependencies {
            let completed: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM steward_tasks WHERE delegation_id=?1 AND goal_plan_id=?2 AND plan_step_id=?3 AND loop_state='done')",
                    params![work.delegation_id, plan_id, dependency],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            ready &= completed;
        }
        if ready {
            return Ok(Some((plan_id, step_id)));
        }
    }
    Ok(None)
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
/// Applies an already-committed coding terminal event. Runner settlement is
/// technical completion only; Goal success requires host evidence.
pub(crate) fn apply_terminal_event(
    connection: &Connection,
    job_id: &str,
    kind: &str,
    run_id: Option<&str>,
) -> Result<(), String> {
    let Some((task_id, _conversation_id, goal_id, goal_status, task_state)): Option<(String, String, String, String, String)> = connection.query_row(
        "SELECT t.id,t.conversation_id,g.id,g.status,t.loop_state FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id JOIN steward_goals g ON g.id=d.goal_id WHERE t.coding_job_id=?1",
        [job_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).optional().map_err(database_error)? else { return Ok(()); };
    if goal_status != "active"
        || !matches!(
            task_state.as_str(),
            "queued" | "dispatching" | "running" | "awaiting_user" | "verifying"
        )
    {
        return Ok(());
    }
    let evaluation = if kind == "settled" {
        Some(super::super::verifier::evaluate_task(
            connection, &task_id, run_id,
        )?)
    } else {
        None
    };
    let state = match kind {
        "failed" | "interrupted" => "failed",
        "outcome_unknown" => "outcome_unknown",
        "settled" => super::super::verifier::map_to_task_state(evaluation.as_ref().expect("evaluated")),
        _ => return Ok(()),
    };
    set_loop_state(connection, &task_id, state, None, None)?;
    connection.execute(
        "INSERT OR IGNORE INTO steward_task_artifacts(id,task_id,kind,reference,terminal_kind,created_at)
         VALUES(?1,?2,'coding_job',?3,?4,?5)",
        params![new_id("artifact"), task_id, job_id, kind, now_iso()],
    ).map_err(database_error)?;
    if state == "done" {
        let _ = queue_dependent_step(connection, &task_id)?;
        super::super::plans::finish_goal_if_complete(connection, &goal_id)?;
    } else if kind == "failed" {
        let _ = replan_after_failure(connection, &task_id)?;
    }
    record_learning_labels(
        connection,
        &task_id,
        job_id,
        kind,
        state,
        evaluation.as_ref(),
    )?;
    Ok(())
}
pub(super) fn record_learning_labels(
    connection: &Connection,
    task_id: &str,
    job_id: &str,
    kind: &str,
    state: &str,
    evaluation: Option<&super::super::execution_contracts::VerifierOutcome>,
) -> Result<(), String> {
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
    if !has_plan_decision {
        return Ok(());
    }
    let verifier_success = match evaluation {
        Some(super::super::execution_contracts::VerifierOutcome::Pass) => Some(true),
        Some(super::super::execution_contracts::VerifierOutcome::Fail) => Some(false),
        _ => None,
    };
    crate::adaptive_improvement::record_outcome(
        connection,
        &plan_decision_id,
        Some(kind == "settled"),
        verifier_success,
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
    let _ = state;
    Ok(())
}
pub(crate) fn replan_after_failure(connection: &Connection, task_id: &str) -> Result<bool, String> {
    let row: Option<(String, String, ActiveWork, String, i64, i64)> = connection.query_row(
        "SELECT t.conversation_id,t.source_id,g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier,
                p.id,p.revision,p.max_replans
         FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id
         JOIN steward_goals g ON g.id=d.goal_id JOIN steward_goal_plans p ON p.id=t.goal_plan_id
         WHERE t.id=?1 AND g.status='active' AND d.status='active' AND d.superseded_by IS NULL",
        [task_id],
        |r| Ok((r.get(0)?, r.get(1)?, ActiveWork { goal_id:r.get(2)?, goal_status:r.get(3)?, delegation_id:r.get(4)?, workspace_id:r.get(5)?, budget_runs:r.get(6)?, budget_ms:r.get(7)?, superseded:r.get::<_, i64>(8)? != 0, ops:r.get(9)?, verifier:r.get(10)? }, r.get(11)?, r.get(12)?, r.get(13)?)),
    ).optional().map_err(database_error)?;
    let Some((conversation_id, source_id, work, prior_plan, revision, max_replans)) = row else {
        return Ok(false);
    };
    if revision > max_replans {
        return Ok(false);
    }
    let new_plan = new_id("goal_plan");
    connection.execute(
        "INSERT INTO steward_goal_plans(id,goal_id,revision,max_replans,created_at) VALUES(?1,?2,?3,?4,?5)",
        params![new_plan, work.goal_id, revision + 1, max_replans, now_iso()],
    ).map_err(database_error)?;
    connection.execute(
        "INSERT INTO steward_plan_steps(plan_id,step_id,ordinal,recipe,verifier,depends_on_json)
         SELECT ?1,step_id,ordinal,recipe,verifier,depends_on_json FROM steward_plan_steps WHERE plan_id=?2",
        params![new_plan, prior_plan],
    ).map_err(database_error)?;
    Ok(queue_task(connection, &work, &conversation_id, &source_id, "continue")?.is_some())
}
pub(super) fn queue_dependent_step(connection: &Connection, task_id: &str) -> Result<bool, String> {
    let row: Option<(String, String, ActiveWork)> = connection.query_row(
        "SELECT t.conversation_id,t.source_id,g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier
         FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id JOIN steward_goals g ON g.id=d.goal_id
         WHERE t.id=?1 AND g.status='active' AND d.status='active' AND d.superseded_by IS NULL",
        [task_id],
        |r| Ok((r.get(0)?, r.get(1)?, ActiveWork { goal_id:r.get(2)?, goal_status:r.get(3)?, delegation_id:r.get(4)?, workspace_id:r.get(5)?, budget_runs:r.get(6)?, budget_ms:r.get(7)?, superseded:r.get::<_, i64>(8)? != 0, ops:r.get(9)?, verifier:r.get(10)? })),
    ).optional().map_err(database_error)?;
    let Some((conversation_id, source_id, work)) = row else {
        return Ok(false);
    };
    Ok(queue_task(connection, &work, &conversation_id, &source_id, "continue")?.is_some())
}
pub(crate) fn set_loop_state(
    connection: &Connection,
    task_id: &str,
    state: &str,
    job_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    let previous: (String, String) = connection
        .query_row(
            "SELECT t.loop_state,g.status FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id WHERE t.id=?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error)?;
    if matches!(previous.0.as_str(), "cancelled" | "outcome_unknown") || previous.1 == "withdrawn" {
        return Ok(());
    }
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
pub(crate) fn completed_recipe(
    connection: &Connection,
    delegation_id: &str,
    recipe: &str,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM steward_tasks t
                JOIN steward_task_plans p ON p.task_id=t.id
                WHERE t.delegation_id=?1 AND p.recipe=?2
                  AND t.loop_state IN ('done','awaiting_user')
            )",
            params![delegation_id, recipe],
            |row| row.get(0),
        )
        .map_err(database_error)
}
pub(crate) fn task_step_recipe(
    connection: &Connection,
    task_id: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT s.recipe FROM steward_tasks t JOIN steward_plan_steps s
             ON s.plan_id=t.goal_plan_id AND s.step_id=t.plan_step_id WHERE t.id=?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
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
           AND d.status='active' AND d.superseded_by IS NULL ORDER BY t.queue_rank,t.rowid LIMIT 1",
        [conversation_id], |r| Ok((ActiveWork { goal_id:r.get(0)?, goal_status:r.get(1)?, delegation_id:r.get(2)?, workspace_id:r.get(3)?, budget_runs:r.get(4)?, budget_ms:r.get(5)?, superseded:r.get::<_, i64>(6)? != 0, ops:r.get(7)?, verifier:r.get(8)? }, r.get(9)?)),
    ).optional().map_err(database_error)
}
pub(crate) fn reorder_queue(
    connection: &Connection,
    conversation_id: &str,
    task_ids: &[String],
) -> Result<(), String> {
    if task_ids.len() > 256 {
        return Err("work_queue_too_large".into());
    }
    let unique = task_ids.iter().collect::<std::collections::HashSet<_>>();
    if unique.len() != task_ids.len() {
        return Err("work_queue_duplicate".into());
    }
    let mut statement = connection
        .prepare(
            "SELECT id FROM steward_tasks WHERE conversation_id=?1 AND loop_state='queued' ORDER BY queue_rank,rowid",
        )
        .map_err(database_error)?;
    let current = statement
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    let current_set = current.iter().collect::<std::collections::HashSet<_>>();
    if current_set != unique {
        return Err("work_queue_stale".into());
    }
    for (index, task_id) in task_ids.iter().enumerate() {
        let changed = connection
            .execute(
                "UPDATE steward_tasks SET queue_rank=?1,revision=revision+1,updated_at=?2 WHERE id=?3 AND conversation_id=?4 AND loop_state='queued'",
                params![(index as i64) + 1, now_iso(), task_id, conversation_id],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("work_queue_stale".into());
        }
    }
    Ok(())
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
