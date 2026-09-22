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
