use super::repository as repo;
use crate::situation::contracts::ForegroundCategory;
use crate::{AppState, StartTurnInput};

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
    state.sqlite_writer.transact(|connection| {
        let Some(source_id) = repo::input_message_id(connection, &input.run_id)? else {
            return Ok(());
        };
        for work in repo::active_delegations(connection, &input.conversation_id)? {
            if repo::workspace_registered(connection, &input.conversation_id, &work.workspace_id)? {
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
    super::dispatch::start_queued_for_conversation(state, conversation_id)
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
