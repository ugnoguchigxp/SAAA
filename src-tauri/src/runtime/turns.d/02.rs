pub(crate) fn prepare_runtime_run(
    state: &AppState,
    input: &StartTurnInput,
) -> Result<String, String> {
    memory::personal_state::worker::interrupt();
    let _policy = state
        .interaction_policy
        .lock()
        .map_err(|_| "Interaction policy lock unavailable".to_string())?;
    let task_mode: String = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT task_mode FROM conversations WHERE id = ?1",
                params![input.conversation_id],
                |row| row.get(0),
            )
            .map_err(|_| "Conversation does not exist".to_string())
    })?;
    validate_conversation_write_target(&input.conversation_id, &task_mode)?;
    if task_mode == "coding" {
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
    } else if input.workspace_path.is_some() {
        return Err("Normal conversation turns cannot include a workspace".to_string());
    }
    if task_mode == "conversation" {
        memory::context_window::validate_current_instruction(input.content.trim())?;
    }
    let route_kind = if task_mode == "coding" {
        "coding.assist"
    } else {
        "conversation.respond"
    };
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let role_routing_enabled = task_mode == "conversation"
            && crate::persistence::load_role_routing_settings(&transaction)?.enabled;
        if task_mode == "conversation"
            && !role_routing_enabled
            && crate::role_routing::repository::disable_drain_in_progress(&transaction)?
        {
            return Err(
                "Role routing is disabled and is still draining previous side effects".into(),
            );
        }
        let active_routing_root = if role_routing_enabled {
            transaction
                .query_row(
                    "SELECT root_id FROM rr_roots WHERE conversation_id=?1 AND phase IN ('responding','draining') ORDER BY started_at_ms LIMIT 1",
                    params![input.conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(database_error)?
        } else {
            None
        };
        let now = now_iso();
        let reasoning_request_message = if input.retry_input_message_id.is_none() {
            crate::larm_voice::frontdesk_repository::claim_reasoning_request(&transaction, input)?
        } else { None };
        let new_message = input.retry_input_message_id.is_none() && reasoning_request_message.is_none();
        let input_message_id = if let Some(message_id) = reasoning_request_message {
            message_id
        } else if let Some(message_id) = input.retry_input_message_id.as_deref() {
            crate::validate_identifier(message_id, "retry input message id")?;
            let retryable: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM conversation_messages m
                       WHERE m.id = ?1 AND m.conversation_id = ?2 AND m.role = 'user'
                         AND m.content = ?3
                         AND EXISTS(
                           SELECT 1 FROM runtime_runs r
                           WHERE r.input_message_id = m.id AND r.status = 'failed'
                         )
                     )",
                    params![message_id, input.conversation_id, input.content.trim()],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !retryable {
                return Err("Only a failed conversation response can be retried".to_string());
            }
            message_id.to_string()
        } else {
            let message_id = new_id("message");
            transaction
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES (?1, ?2, 'user', ?3, ?4)",
                    params![message_id, input.conversation_id, input.content.trim(), now],
                )
                .map_err(database_error)?;
            message_id
        };
        if task_mode == "conversation" && new_message {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            let follow_up = match active_routing_root.as_deref() {
                Some(root_id) => crate::role_routing::signals::classify_active_follow_up(
                    input.content.trim(),
                    root_id,
                ),
                None => crate::role_routing::signals::classify_follow_up(input.content.trim()),
            };
            let feedback_kind = match follow_up {
                crate::role_routing::signals::SignalKind::AnswerChallenge => {
                    Some("answer_challenge")
                }
                crate::role_routing::signals::SignalKind::ExplicitPositive => {
                    Some("explicit_positive")
                }
                crate::role_routing::signals::SignalKind::ExplicitNegative => {
                    Some("explicit_negative")
                }
                _ => None,
            };
            if let Some(feedback_kind) = feedback_kind {
                crate::role_routing::repository::record_feedback_for_latest_answer(
                    &transaction,
                    &input.conversation_id,
                    &input_message_id,
                    feedback_kind,
                    0,
                    input.content.trim().len(),
                    now_ms,
                )?;
            }
            if let Some(active_root_id) = active_routing_root.as_deref() {
                let event = match follow_up {
                    crate::role_routing::signals::SignalKind::Status
                    | crate::role_routing::signals::SignalKind::ExplicitPositive
                    | crate::role_routing::signals::SignalKind::ExplicitNegative => None,
                    crate::role_routing::signals::SignalKind::Cancel => {
                        Some(crate::role_routing::reducer::Event::Cancel)
                    }
                    _ => Some(crate::role_routing::reducer::Event::InputBarrier),
                };
                if let Some(event) = event {
                    crate::role_routing::coordinator::apply_in_transaction(
                        &transaction,
                        active_root_id,
                        event,
                        now_ms,
                    )?;
                }
            }
        }
        transaction
            .execute(
                "INSERT INTO runtime_runs(
                   id,conversation_id,route_kind,status,started_at,supervisor_version,input_message_id
                 ) VALUES(?1,?2,?3,'running',?4,?5,?6)",
                params![
                    input.run_id,
                    input.conversation_id,
                    route_kind,
                    now,
                    if task_mode == "coding" {
                        Some(crate::runtime::contracts::SUPERVISOR_VERSION)
                    } else {
                        None
                    },
                    input_message_id,
                ],
            )
            .map_err(database_error)?;
        crate::runtime::context::scope::resolve(
            &transaction,
            input,
            &input_message_id,
            new_message,
        )?;
        if task_mode == "conversation" {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            let routing_started = crate::role_routing::repository::record_provider_turn_start_in_transaction(
                &transaction,
                &input.run_id,
                &input.conversation_id,
                &input.input_origin,
                input.source_id.as_deref(),
                &input.presentation_mode,
                now_ms,
            )?;
            if routing_started {
                let another_active: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM rr_roots WHERE conversation_id=?1 AND root_id<>?2 AND phase IN ('responding','draining'))",
                        params![input.conversation_id, input.run_id],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                if !another_active {
                    crate::role_routing::coordinator::apply_in_transaction(
                        &transaction,
                        &input.run_id,
                        crate::role_routing::reducer::Event::Start,
                        now_ms,
                    )?;
                }
            }
        }
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?1, title = COALESCE(title, ?2) WHERE id = ?3",
                params![
                    now,
                    bounded_text(input.content.trim(), 60),
                    input.conversation_id
                ],
            )
            .map_err(database_error)?;
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
