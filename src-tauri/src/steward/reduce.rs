use super::repository as repo;
use crate::runtime::agent_tools::AgentToolCall;
use crate::situation::contracts::ForegroundCategory;
use crate::{new_id, AppState, StartTurnInput};
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
        let Some(work) = repo::active_delegation(connection, &input.conversation_id)? else {
            return Ok(());
        };
        if !repo::workspace_registered(connection, &input.conversation_id, &work.workspace_id)? {
            return Ok(());
        }
        let Some(source_id) = repo::input_message_id(connection, &input.run_id)? else {
            return Ok(());
        };
        let _ = repo::queue_task(connection, &work, &input.conversation_id, &source_id, kind)?;
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
                &["委任は撤回済みです".into()],
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
    let request = repo::start_request();
    if repo::request_forbidden(request) {
        return Ok(());
    }
    let prepared = state.sqlite_writer.write(|connection| {
        let Some(work) = repo::active_delegation(connection, &input.conversation_id)? else {
            return Ok(None);
        };
        if repo::budget_exceeded(connection, &work)? {
            if let Some((task_id, _)) = repo::queued_task(connection, &work.delegation_id)? {
                repo::set_loop_state(connection, &task_id, "awaiting_user", None, Some("budget"))?;
            }
            return Ok(None);
        }
        if !repo::workspace_registered(connection, &input.conversation_id, &work.workspace_id)? {
            return Ok(None);
        }
        let Some((task_id, _)) = repo::queued_task(connection, &work.delegation_id)? else {
            return Ok(None);
        };
        Ok(Some((work.workspace_id, task_id)))
    })?;
    let Some((workspace_id, task_id)) = prepared else {
        return Ok(());
    };
    if !repo::coding_enabled(state)? {
        return Ok(());
    }
    let call = AgentToolCall {
        id: new_id("call"),
        name: "coding_start".into(),
        arguments: json!({"workspaceId": workspace_id, "request": request}).to_string(),
    };
    let result = crate::coding::service::execute(state, input, &call);
    state.sqlite_writer.write(|connection| {
        match result {
            Ok(value) => {
                let job = value["jobId"].as_str();
                repo::set_loop_state(connection, &task_id, "running", job, None)?;
            }
            Err(error) => {
                repo::set_loop_state(connection, &task_id, "queued", None, Some(&error))?;
            }
        }
        Ok(())
    })
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
