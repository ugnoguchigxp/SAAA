use super::{
    contracts::{GoalProposal, PlanStep, TaskPlan, Verifier, MAX_ACTIVE_GOALS},
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
    register_with_options(
        connection,
        conversation_id,
        workspace_id,
        success_condition,
        "",
        "test_report_obtained",
        "read_test",
        3,
        60_000,
        "both",
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn register_with_options(
    connection: &Connection,
    conversation_id: &str,
    workspace_id: &str,
    success_condition: &str,
    summary: &str,
    verifier: &str,
    ops: &str,
    budget_runs: u8,
    budget_ms: u64,
    notify: &str,
) -> Result<Value, String> {
    if workspace_id.is_empty() || success_condition.trim().is_empty() {
        return Err("steward_register_invalid".into());
    }
    if summary.chars().count() > 2_000
        || !matches!(
            verifier,
            "test_report_obtained" | "tests_pass" | "user_confirmation_required"
        )
        || !matches!(ops, "read" | "test_run" | "read_test")
        || budget_runs == 0
        || budget_runs > 16
        || budget_ms == 0
        || budget_ms > 3_600_000
        || !matches!(notify, "both" | "silent" | "speak")
    {
        return Err("steward_register_invalid".into());
    }
    if !workspace_registered(connection, conversation_id, workspace_id)? {
        return Err("workspace_required".into());
    }
    let active = super::admission::incomplete_goal_count(connection, conversation_id)?;
    if active as usize >= MAX_ACTIVE_GOALS {
        return Err("active_goal_limit".into());
    }
    let now = now_iso();
    let goal_id = new_id("goal");
    let delegation_id = new_id("delegation");
    connection
        .execute(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,superseded_by,summary,revision,verifier)
             VALUES(?1,?2,'user_explicit',?3,'active',?4,NULL,?5,1,?6)",
            params![goal_id, conversation_id, success_condition.trim(), now, summary.trim(), verifier],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at,superseded_by)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'active',?9,NULL)",
            params![delegation_id, goal_id, conversation_id, workspace_id, ops, budget_runs, budget_ms.min(i64::MAX as u64) as i64, notify, now],
        )
        .map_err(database_error)?;
    persist_default_goal_plan(connection, &goal_id, ops, verifier)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO steward_goal_progress(goal_id,work_status,revision,updated_at)
             VALUES(?1,'queued',1,?2)",
            params![goal_id, now],
        )
        .map_err(database_error)?;
    Ok(json!({"goalId":goal_id,"delegationId":delegation_id,"status":"active"}))
}

fn persist_default_goal_plan(
    connection: &Connection,
    goal_id: &str,
    ops: &str,
    verifier: &str,
) -> Result<(), String> {
    let verifier = match verifier {
        "test_report_obtained" => Verifier::TestReportObtained,
        "tests_pass" => Verifier::TestsPass,
        _ => Verifier::UserConfirmationRequired,
    };
    let steps = match ops {
        "read" => vec![PlanStep {
            id: "read".into(),
            depends_on: vec![],
            verifier,
            recipe: Some("read".into()),
            capability: None,
            verifier_input: None,
        }],
        "test_run" => vec![PlanStep {
            id: "test".into(),
            depends_on: vec![],
            verifier,
            recipe: Some("test_run".into()),
            capability: None,
            verifier_input: None,
        }],
        "read_test" => vec![
            PlanStep {
                id: "read".into(),
                depends_on: vec![],
                verifier: verifier.clone(),
                recipe: Some("read".into()),
                capability: None,
                verifier_input: None,
            },
            PlanStep {
                id: "test".into(),
                depends_on: vec!["read".into()],
                verifier,
                recipe: Some("test_run".into()),
                capability: None,
                verifier_input: None,
            },
        ],
        _ => return Err("steward_plan_invalid".into()),
    };
    let plan = TaskPlan {
        steps,
        max_replans: 2,
    };
    plan.validate().map_err(str::to_string)?;
    let id = new_id("goal_plan");
    connection.execute(
        "INSERT INTO steward_goal_plans(id,goal_id,revision,max_replans,created_at) VALUES(?1,?2,1,?3,?4)",
        params![id, goal_id, i64::from(plan.max_replans), now_iso()],
    ).map_err(database_error)?;
    for (ordinal, step) in plan.steps.iter().enumerate() {
        let recipe = if step.id == "read" {
            "read"
        } else {
            "test_run"
        };
        let verifier = match &step.verifier {
            Verifier::TestReportObtained => "test_report_obtained",
            Verifier::TestsPass => "tests_pass",
            Verifier::UserConfirmationRequired => "user_confirmation_required",
        };
        connection.execute(
            "INSERT INTO steward_plan_steps(plan_id,step_id,ordinal,recipe,verifier,depends_on_json) VALUES(?1,?2,?3,?4,?5,?6)",
            params![id, step.id, ordinal as i64, recipe, verifier, serde_json::to_string(&step.depends_on).map_err(|_| "steward_plan_invalid")?],
        ).map_err(database_error)?;
    }
    Ok(())
}

/// Accepts a model proposal only after the host has bound it to an existing
/// user message.  The proposal text itself is intentionally not trusted as an
/// authority grant.
pub(crate) fn propose(
    connection: &Connection,
    conversation_id: &str,
    proposal: &GoalProposal,
) -> Result<Value, String> {
    let result = super::intake::classify(
        connection,
        conversation_id,
        &proposal.source_message_id,
        proposal,
    )?;
    Ok(super::admission::json_result(result))
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
            "SELECT t.id,t.loop_state,t.dedupe_key,t.coding_job_id,g.status,g.id,g.summary,g.verifier,d.workspace_id,d.ops,d.budget_runs,d.budget_ms,d.notify,
                    (SELECT r.delivery_state FROM steward_reports r WHERE r.task_id=t.id ORDER BY r.rowid DESC LIMIT 1),
                    (SELECT r.speech_state FROM steward_reports r WHERE r.task_id=t.id ORDER BY r.rowid DESC LIMIT 1),
                    COALESCE((SELECT group_concat(a.reference, char(31)) FROM steward_task_artifacts a WHERE a.task_id=t.id), ''),
                    (SELECT o.outcome FROM steward_verifier_outcomes o WHERE o.task_id=t.id ORDER BY o.rowid DESC LIMIT 1),
                    (SELECT o.reason_code FROM steward_verifier_outcomes o WHERE o.task_id=t.id ORDER BY o.rowid DESC LIMIT 1),
                    t.revision,t.queue_rank,t.created_at,t.updated_at
             FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.conversation_id=?1
             ORDER BY t.rowid",
        )
        .map_err(database_error)?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            let artifacts: String = row.get(15)?;
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
                "deliveryState": row.get::<_, Option<String>>(13)?,
                "speechState": row.get::<_, Option<String>>(14)?,
                "artifactRefs": artifacts.split('\u{1f}').filter(|value| !value.is_empty()).collect::<Vec<_>>(),
                "verifierOutcome": row.get::<_, Option<String>>(16)?,
                "evidenceReason": row.get::<_, Option<String>>(17)?,
                "revision": row.get::<_, i64>(18)?,
                "queueRank": row.get::<_, i64>(19)?,
                "createdAt": row.get::<_, String>(20)?,
                "updatedAt": row.get::<_, String>(21)?,
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

fn next_ready_plan_step(
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
        Some(super::verifier::evaluate_task(
            connection, &task_id, run_id,
        )?)
    } else {
        None
    };
    let state = match kind {
        "failed" | "interrupted" => "failed",
        "outcome_unknown" => "outcome_unknown",
        "settled" => super::verifier::map_to_task_state(evaluation.as_ref().expect("evaluated")),
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
        super::plans::finish_goal_if_complete(connection, &goal_id)?;
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

fn record_learning_labels(
    connection: &Connection,
    task_id: &str,
    job_id: &str,
    kind: &str,
    state: &str,
    evaluation: Option<&super::execution_contracts::VerifierOutcome>,
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
        Some(super::execution_contracts::VerifierOutcome::Pass) => Some(true),
        Some(super::execution_contracts::VerifierOutcome::Fail) => Some(false),
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

fn queue_dependent_step(connection: &Connection, task_id: &str) -> Result<bool, String> {
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
