use super::repository as repo;
use crate::situation::contracts::ForegroundCategory;
use crate::{AppState, StartTurnInput};
use rusqlite::OptionalExtension;
use serde_json::json;

pub(crate) fn on_user_message(state: &AppState, input: &StartTurnInput) {
    if !crate::memory::control_plane::memory_enabled() {
        return;
    }
    let _ = inspect_coding_transition(state, &input.conversation_id);
    let _ = reduce_message(state, input);
}

fn reduce_message(state: &AppState, input: &StartTurnInput) -> Result<(), String> {
    state
        .sqlite_writer
        .write(|connection| super::report::publish(state, connection, &input.conversation_id))?;
    let text = input.content.trim();
    let (start, cont) = repo::triggers();
    if text == start {
        queue(state, input, "start")?;
        try_start(state, input)
    } else if text == cont {
        continue_task(state, input)
    } else {
        Ok(())
    }
}

fn queue(state: &AppState, input: &StartTurnInput, kind: &str) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let Some(source_id) = repo::input_message_id(connection, &input.run_id)? else {
            return Ok(());
        };
        for work in repo::active_delegations(connection, &input.conversation_id)? {
            if repo::workspace_registered(connection, &input.conversation_id, &work.workspace_id)? {
                let _ =
                    repo::queue_task(connection, &work, &input.conversation_id, &source_id, kind)?;
            }
        }
        Ok(())
    })
}

fn continue_task(state: &AppState, input: &StartTurnInput) -> Result<(), String> {
    let withdrawn = state.sqlite_writer.write(|connection| {
        let Some(work) = repo::latest_work(connection, &input.conversation_id)? else {
            return Ok(false);
        };
        if work.goal_status == "withdrawn" || work.superseded {
            super::report::queue_terminals(
                state,
                connection,
                &input.conversation_id,
                &[repo::TerminalReport {
                    task_id: format!("withdrawn-{}", work.goal_id),
                    task_revision: 1,
                    goal_id: work.goal_id,
                    notify: "both".into(),
                    digest: "委任は撤回済みです".into(),
                }],
            )?;
            return Ok(true);
        }
        Ok(false)
    })?;
    if withdrawn {
        return Ok(());
    }
    inspect_jobs(state, &input.conversation_id)
}

fn try_start(state: &AppState, input: &StartTurnInput) -> Result<(), String> {
    start_queued(state, input)
}

/// Executes only the fixed read/test recipe against a persisted user source.
/// Proposal text is never promoted into an executable prompt or permission.
pub(crate) fn start_queued(state: &AppState, input: &StartTurnInput) -> Result<(), String> {
    start_queued_for_conversation(state, &input.conversation_id)
}

pub(crate) fn start_queued_for_conversation(
    state: &AppState,
    conversation_id: &str,
) -> Result<(), String> {
    let prepared = state.sqlite_writer.write(|connection| {
        let transaction = connection.unchecked_transaction().map_err(crate::database_error)?;
        let Some((work, task_id)) = repo::next_queued_work(&transaction, conversation_id)?
        else {
            transaction.commit().map_err(crate::database_error)?;
            return Ok(None);
        };
        if repo::budget_exceeded(&transaction, &work)? {
            repo::set_loop_state(&transaction, &task_id, "awaiting_user", None, Some("budget"))?;
            transaction.commit().map_err(crate::database_error)?;
            return Ok(None);
        }
        if !repo::workspace_registered(&transaction, conversation_id, &work.workspace_id)? {
            transaction.commit().map_err(crate::database_error)?;
            return Ok(None);
        }
        if !repo::claim_dispatch(&transaction, &task_id)? {
            transaction.commit().map_err(crate::database_error)?;
            return Ok(None);
        }
        let plan = select_plan_recipe(&transaction, &work, &task_id, now_ms())?;
        if repo::request_forbidden(plan.request) {
            return Err("steward_plan_forbidden".into());
        }
        repo::persist_task_plan(
            &transaction,
            &task_id,
            plan.id,
            plan.request,
            plan.selection_mode,
            plan.policy_revision,
        )?;
        crate::adaptive_improvement::record_decision(
            &transaction,
            &crate::adaptive_improvement::DecisionObservation {
                id: format!("ai-plan-{task_id}"),
                domain: crate::adaptive_improvement::Domain::Plan,
                scope_key: work.goal_id.clone(),
                event_seq: 0,
                policy_revision: plan.policy_revision,
                candidate_fingerprint: crate::adaptive_improvement::fingerprint_for(&plan.eligible),
                eligible_candidates: plan.eligible,
                selected: plan.id.into(),
                selection_mode: plan.selection_mode.into(),
                source_refs_json: json!({"goalId": work.goal_id, "taskId": task_id, "delegationId": work.delegation_id}).to_string(),
            },
            now_ms(),
        )?;
        transaction.commit().map_err(crate::database_error)?;
        Ok(Some((work.workspace_id, task_id, plan.request)))
    })?;
    let Some((workspace_id, task_id, request)) = prepared else {
        return Ok(());
    };
    if !repo::coding_enabled(state)? {
        return Ok(());
    }
    if !repo::delegated_profile_available(state)? {
        return state.sqlite_writer.write(|connection| {
            repo::set_loop_state(
                connection,
                &task_id,
                "awaiting_user",
                None,
                Some("delegated_profile_required"),
            )
        });
    }
    let result = crate::coding::service::execute_delegated(
        state,
        conversation_id,
        &task_id,
        &workspace_id,
        request,
    );
    state.sqlite_writer.write(|connection| {
        match result {
            Ok(value) => {
                let job = value["jobId"].as_str();
                repo::set_loop_state(connection, &task_id, "running", job, None)?;
                repo::settle_dispatch(connection, &task_id, Some(&value), false)?;
            }
            Err(error) => {
                repo::set_loop_state(connection, &task_id, "queued", None, Some(&error))?;
                repo::settle_dispatch(connection, &task_id, None, true)?;
            }
        }
        Ok(())
    })
}

/// Schedule is an independent event source: it dispatches one already queued
/// task by its durable ids, rather than fabricating a conversation turn.
pub(crate) fn dispatch_scheduled(
    state: &AppState,
    task_id: &str,
    delegation_id: &str,
) -> Result<serde_json::Value, String> {
    if !repo::coding_enabled(state)? {
        return Err("coding_disabled".into());
    }
    if !repo::delegated_profile_available(state)? {
        return Err("delegated_profile_required".into());
    }
    let prepared = state.sqlite_writer.write(|connection| {
        let tx = connection.unchecked_transaction().map_err(crate::database_error)?;
        let row: Option<(String, repo::ActiveWork)> = tx.query_row(
            "SELECT t.conversation_id,g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier
             FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.id=?1 AND d.id=?2 AND t.loop_state='queued' AND g.status='active' AND g.superseded_by IS NULL
               AND d.status='active' AND d.superseded_by IS NULL",
            rusqlite::params![task_id, delegation_id],
            |r| Ok((r.get(0)?, repo::ActiveWork { goal_id:r.get(1)?, goal_status:r.get(2)?, delegation_id:r.get(3)?, workspace_id:r.get(4)?, budget_runs:r.get(5)?, budget_ms:r.get(6)?, superseded:r.get::<_,i64>(7)? != 0, ops:r.get(8)?, verifier:r.get(9)? })),
        ).optional().map_err(crate::database_error)?;
        let Some((conversation_id, work)) = row else { return Err("delegated_task_unavailable".into()); };
        if repo::budget_exceeded(&tx, &work)? { return Err("budget".into()); }
        if !repo::workspace_registered(&tx, &conversation_id, &work.workspace_id)? { return Err("workspace_required".into()); }
        if !repo::claim_dispatch(&tx, task_id)? { return Err("dispatch_unavailable".into()); }
        let plan = select_plan_recipe(&tx, &work, task_id, now_ms())?;
        if repo::request_forbidden(plan.request) { return Err("steward_plan_forbidden".into()); }
        repo::persist_task_plan(&tx, task_id, plan.id, plan.request, plan.selection_mode, plan.policy_revision)?;
        tx.commit().map_err(crate::database_error)?;
        Ok((conversation_id, work.workspace_id, plan.request))
    })?;
    let (conversation_id, workspace_id, request) = prepared;
    let result = crate::coding::service::execute_delegated(
        state,
        &conversation_id,
        task_id,
        &workspace_id,
        request,
    );
    state.sqlite_writer.write(|connection| match &result {
        Ok(value) => {
            repo::set_loop_state(
                connection,
                task_id,
                "running",
                value["jobId"].as_str(),
                None,
            )?;
            repo::settle_dispatch(connection, task_id, Some(value), false)
        }
        Err(error) => {
            repo::set_loop_state(connection, task_id, "queued", None, Some(error))?;
            repo::settle_dispatch(connection, task_id, None, true)
        }
    })?;
    result
}

struct PlanRecipe {
    id: &'static str,
    request: &'static str,
    eligible: Vec<String>,
    selection_mode: &'static str,
    policy_revision: i64,
}

const READ_REQUEST: &str = "Inspect the existing failure evidence in this workspace and report causes. Do not run tests or change files.";
const TEST_REQUEST: &str = "Run the relevant existing tests in this workspace and report their result. Do not change files.";
const READ_TEST_REQUEST: &str = "Inspect failing tests in this workspace. Read logs, run the relevant existing tests, and report causes. Do not change files.";

/// Only recipes that are subsets of the persisted delegation are eligible. This keeps learning
/// from creating authority: it can skip a redundant read phase, but never add a write, network,
/// or unapproved test action.
fn select_plan_recipe(
    connection: &rusqlite::Connection,
    work: &repo::ActiveWork,
    _task_id: &str,
    now: i64,
) -> Result<PlanRecipe, String> {
    let (rules, eligible) = match (work.ops.as_str(), work.verifier.as_str()) {
        ("read", _) => ("read", vec!["read".to_string()]),
        ("test_run", _) => ("test_run", vec!["test_run".to_string()]),
        ("read_test", _) if repo::completed_recipe(connection, &work.delegation_id, "read")? => {
            ("test_run", vec!["test_run".to_string()])
        }
        ("read_test", _) => ("read", vec!["read".to_string()]),
        _ => return Err("steward_plan_invalid".into()),
    };
    let settings = crate::persistence::load_role_routing_settings(connection)?;
    let (selected, selection_mode, policy_revision) =
        if settings.adaptive_improvement.enabled && settings.adaptive_improvement.plan {
            crate::adaptive_improvement::choose(
                connection,
                crate::adaptive_improvement::Domain::Plan,
                &work.goal_id,
                &eligible,
                rules,
                now,
            )?
        } else {
            (rules.to_string(), "rules", 0)
        };
    let request = match selected.as_str() {
        "read" => READ_REQUEST,
        "test_run" => TEST_REQUEST,
        "read_test" => READ_TEST_REQUEST,
        _ => return Err("steward_plan_invalid".into()),
    };
    Ok(PlanRecipe {
        id: match selected.as_str() {
            "read" => "read",
            "test_run" => "test_run",
            _ => "read_test",
        },
        request,
        eligible,
        selection_mode,
        policy_revision,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod adaptive_plan_tests {
    #[test]
    fn registered_plan_recipes_are_strict_subsets_of_delegated_ops() {
        let candidates = |ops: &str, read_completed: bool| match (ops, read_completed) {
            ("read", _) => vec!["read"],
            ("test_run", _) => vec!["test_run"],
            ("read_test", true) => vec!["test_run"],
            ("read_test", false) => vec!["read"],
            _ => vec![],
        };
        assert_eq!(candidates("read", false), ["read"]);
        assert_eq!(candidates("test_run", false), ["test_run"]);
        assert_eq!(candidates("read_test", false), ["read"]);
        assert_eq!(candidates("read_test", true), ["test_run"]);
        assert!(candidates("write", false).is_empty());
    }
}

pub(crate) fn inspect_coding_transition(
    state: &AppState,
    conversation_id: &str,
) -> Result<(), String> {
    if !crate::memory::control_plane::memory_enabled() {
        return Ok(());
    }
    let current = match state.situation.foreground_category() {
        ForegroundCategory::Coding => "Coding",
        other => {
            store_foreground(state, conversation_id, format!("{other:?}"))?;
            return Ok(());
        }
    };
    let previous = state
        .sqlite_readers
        .read(|connection| repo::last_foreground(connection, conversation_id))?;
    store_foreground(state, conversation_id, current.into())?;
    if previous.as_deref() == Some("Coding") || previous.is_none() {
        return Ok(());
    }
    inspect_jobs(state, conversation_id)
}

fn store_foreground(
    state: &AppState,
    conversation_id: &str,
    category: String,
) -> Result<(), String> {
    state
        .sqlite_writer
        .write(|connection| repo::store_foreground(connection, conversation_id, &category))
}

fn inspect_jobs(state: &AppState, conversation_id: &str) -> Result<(), String> {
    let jobs = state
        .sqlite_readers
        .read(|connection| repo::inspectable_jobs(connection, conversation_id))?;
    for (_task, job) in jobs {
        let _ = state.sqlite_readers.read(|connection| {
            crate::coding::repository::inspect(connection, conversation_id, &job, 0, 1)
        });
    }
    state
        .sqlite_writer
        .write(|connection| super::report::publish(state, connection, conversation_id))
}
