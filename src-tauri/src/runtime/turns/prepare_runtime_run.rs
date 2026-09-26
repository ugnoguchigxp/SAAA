use super::*;

pub(crate) fn prepare_runtime_run(state: &AppState, input: &StartTurnInput) -> Result<String, String> {
    let task_mode: String = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT task_mode FROM conversations WHERE id = ?1",
                params![input.conversation_id],
                |row| row.get(0),
            )
            .map_err(|_| "Conversation does not exist".to_string())
    })?;
    if task_mode != "coding" {
        return Err("The legacy conversation runtime was removed pending replacement".into());
    }
    validate_conversation_write_target(&input.conversation_id, &task_mode)?;
    let workspace = input
        .workspace_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Select a workspace before starting a Codex turn".to_string())?;
    let workspace = fs::canonicalize(workspace)
        .map_err(|_| "The selected Codex workspace does not exist".to_string())?;
    if !workspace.is_dir() {
        return Err("The selected Codex workspace is not a directory".to_string());
    }
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let now = now_iso();
        let input_message_id = if let Some(message_id) = input.retry_input_message_id.as_deref() {
            crate::validate_identifier(message_id, "retry input message id")?;
            let retryable: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_messages m WHERE m.id=?1 AND m.conversation_id=?2 AND m.role='user' AND m.content=?3 AND EXISTS(SELECT 1 FROM runtime_runs r WHERE r.input_message_id=m.id AND r.status='failed'))",
                params![message_id, input.conversation_id, input.content.trim()],
                |row| row.get(0),
            ).map_err(database_error)?;
            if !retryable {
                return Err("Only a failed response can be resumed".into());
            }
            message_id.to_string()
        } else {
            let message_id = new_id("message");
            transaction.execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",
                params![message_id, input.conversation_id, input.content.trim(), now],
            ).map_err(database_error)?;
            message_id
        };
        transaction.execute(
            "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at,supervisor_version,input_message_id) VALUES(?1,?2,'coding.assist','running',?3,?4,?5)",
            params![input.run_id, input.conversation_id, now, crate::runtime::contracts::SUPERVISOR_VERSION, input_message_id],
        ).map_err(database_error)?;
        crate::runtime::context::scope::resolve(&transaction, input, &input_message_id, false)?;
        transaction.execute(
            "UPDATE conversations SET updated_at=?1,title=COALESCE(title,?2) WHERE id=?3",
            params![now, bounded_text(input.content.trim(), 60), input.conversation_id],
        ).map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
        Ok(())
    })?;
    Ok(task_mode)
}
#[cfg(test)]
pub(crate) fn finish_runtime_run(
    state: &AppState,
    run_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let changed = connection
            .execute(
                "UPDATE runtime_runs
                 SET status = ?1, error_message = ?2, completed_at = ?3
                 WHERE id = ?4 AND status = 'running'",
                params![status, error.map(redact_runtime_text), now_iso(), run_id],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Runtime run was already finalized".to_string());
        }
        Ok(())
    })
}
#[cfg(test)]
mod required_context_failure_code_tests {
    use super::*;

    #[test]
    fn required_context_recovery_messages_keep_distinct_public_codes() {
        let overflow = TurnExecutionFailure::configuration(
            "Required context does not fit this provider. Narrow the task scope.",
        );
        assert!(matches!(
            public_failure_code(&overflow),
            RuntimeFailureCode::RequiredContextOverflow
        ));
        let scope = TurnExecutionFailure::configuration(
            "Context scope changed before dispatch. Choose the intended task.",
        );
        assert!(matches!(
            public_failure_code(&scope),
            RuntimeFailureCode::ContextScopeChanged
        ));
        let unresolved =
            TurnExecutionFailure::configuration("Context scope could not be resolved: unknown");
        assert!(matches!(
            public_failure_code(&unresolved),
            RuntimeFailureCode::ContextScopeChanged
        ));
    }
}
