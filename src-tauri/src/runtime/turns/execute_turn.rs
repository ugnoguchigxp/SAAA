use super::*;
pub(crate) async fn execute_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    codex_policy_override: Option<crate::runtime::contracts::RunSupervisionPolicy>,
) -> Result<(), TurnExecutionFailure> {
    let task_mode = match prepare_runtime_run(state, input) {
        Ok(task_mode) => task_mode,
        Err(message) => {
            let _ = on_event.send(RuntimeEvent::Failed {
                run_id: input.run_id.clone(),
                code: RuntimeFailureCode::RuntimeError,
                message: redact_runtime_text(&message),
                recovery: "Review the conversation and runtime state, then retry.".to_string(),
            });
            return Err(TurnExecutionFailure::unsupervised(
                crate::runtime::contracts::RunFailureCode::InternalError,
                message,
            ));
        }
    };
    crate::steward::on_user_message(state, input);
    state
        .situation
        .set_conversation_state(if task_mode == "coding" {
            situation::contracts::ConversationState::AgentRunning
        } else {
            situation::contracts::ConversationState::ModelRunning
        });
    if task_mode == "coding" {
        let result = execute_codex_turn(
            state,
            input,
            on_event,
            cancellation.clone(),
            codex_policy_override,
        )
        .await;
        if let Err(error) = &result {
            if !error.finalized {
                let cancelled = cancellation.is_cancelled()
                    || error.code == crate::runtime::contracts::RunFailureCode::UserCancelled;
                finish_supervised_runtime_run(
                    state,
                    &input.run_id,
                    if cancelled { "cancelled" } else { "failed" },
                    Some(if cancelled {
                        crate::runtime::contracts::RunFailureCode::UserCancelled
                    } else {
                        error.code
                    }),
                    error.supervisor_version,
                    error.last_progress_at.as_deref(),
                    Some(&error.message),
                )
                .map_err(|message| {
                    TurnExecutionFailure::unsupervised(
                        crate::runtime::contracts::RunFailureCode::InternalError,
                        message,
                    )
                })?;
                send_runtime_terminal_event(on_event, &input.run_id, error, cancelled);
            }
        }
        state
            .situation
            .set_conversation_state(situation::contracts::ConversationState::Idle);
        return result.map(|_| ());
    }

    if crate::runtime::capability_commands::handle_user_turn(
        state,
        input,
        on_event,
        cancellation.clone(),
    )
    .await?
    {
        state
            .situation
            .set_conversation_state(situation::contracts::ConversationState::Idle);
        return Ok(());
    }

    state
        .situation
        .set_conversation_state(situation::contracts::ConversationState::Idle);
    Err(TurnExecutionFailure::unsupervised(
        crate::runtime::contracts::RunFailureCode::InternalError,
        "The legacy conversation runtime was removed pending replacement".into(),
    ))
}
pub(super) async fn wait_for_role_routing_dispatch(
    state: &AppState,
    input: &StartTurnInput,
    cancellation: Arc<RunCancellation>,
) -> Result<(), TurnExecutionFailure> {
    loop {
        if cancellation.is_cancelled() {
            return Err(TurnExecutionFailure::provider(
                crate::ProviderFailureKind::Cancelled,
                "Cancelled by user".into(),
            ));
        }
        let root = state.sqlite_readers.read(|connection| {
            connection
                .query_row(
                    "SELECT phase,deadline_at_ms FROM rr_roots WHERE root_id=?1",
                    [&input.run_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())
        })?;
        let Some((phase, deadline_at_ms)) = root else {
            return Ok(());
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(i64::MAX);
        if deadline_at_ms.is_some_and(|deadline| deadline <= now_ms)
            && matches!(phase.as_str(), "queued" | "responding" | "draining")
        {
            state.sqlite_writer.write(|connection| {
                crate::role_routing::coordinator::apply(
                    connection,
                    &input.run_id,
                    crate::role_routing::reducer::Event::Fail,
                    now_ms,
                )?;
                let _ = crate::role_routing::recovery::claim_next_queued(connection, now_ms)?;
                Ok(())
            })?;
            return Err(TurnExecutionFailure::provider(
                crate::ProviderFailureKind::Timeout,
                "Role-routing root reached its deadline before provider dispatch".into(),
            ));
        }
        match phase.as_str() {
            "responding" => return Ok(()),
            // `draining` is a durable input/cancel barrier. A task that has not dispatched yet
            // must wait for Release/Resume instead of starting provider I/O that cannot be
            // adopted. An already-running provider never passes through this pre-dispatch wait.
            "queued" => {
                let cancelled_run = state.sqlite_writer.write(|connection| {
                    expire_stale_input_barrier(connection, &input.conversation_id, now_ms)
                })?;
                if let Some(run_id) = cancelled_run {
                    if let Ok(active) = state.active_runs.lock() {
                        if let Some(cancellation) = active.get(&run_id) {
                            cancellation.cancel();
                        }
                    }
                    state.streaming_tts.cancel(&run_id);
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            "draining" => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            "cancelled" => {
                return Err(TurnExecutionFailure::provider(
                    crate::ProviderFailureKind::Cancelled,
                    "Role-routing root was cancelled before dispatch".into(),
                ));
            }
            "failed" | "completed" => {
                return Err(TurnExecutionFailure::configuration(
                    "Role-routing root became terminal before provider dispatch",
                ));
            }
            _ => {
                return Err(TurnExecutionFailure::configuration(
                    "Role-routing root has an invalid dispatch phase",
                ));
            }
        }
    }
}
/// Fails a barrier that outlived its immutable classification budget, then promotes one FIFO
/// receipt. This prevents a crashed or unavailable classifier from deadlocking the conversation.
pub(crate) fn expire_stale_input_barrier(
    connection: &mut rusqlite::Connection,
    conversation_id: &str,
    now_ms: i64,
) -> Result<Option<String>, String> {
    let stale: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT r.root_id,r.runtime_run_id
             FROM rr_roots r JOIN rr_policy_versions p ON p.id=r.policy_id
             WHERE r.conversation_id=?1 AND r.phase='draining'
               AND COALESCE((SELECT MAX(e.created_at_ms) FROM rr_events e WHERE e.root_id=r.root_id AND e.kind='input_barrier'),r.started_at_ms)
                   + COALESCE(json_extract(p.config_json,'$.limits.classificationTimeoutMs'),1500) <= ?2
             ORDER BY r.started_at_ms,r.root_id LIMIT 1",
            rusqlite::params![conversation_id, now_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((root_id, runtime_run_id)) = stale else {
        return Ok(None);
    };
    crate::role_routing::coordinator::apply(
        connection,
        &root_id,
        crate::role_routing::reducer::Event::Fail,
        now_ms,
    )?;
    let _ = crate::role_routing::recovery::claim_next_queued(connection, now_ms)?;
    Ok(runtime_run_id)
}
pub(crate) fn send_runtime_terminal_event(
    on_event: &dyn RuntimeEventSender,
    run_id: &str,
    error: &TurnExecutionFailure,
    cancelled: bool,
) {
    if cancelled {
        let _ = on_event.send(RuntimeEvent::Cancelled {
            run_id: run_id.to_string(),
        });
    } else {
        let _ = on_event.send(RuntimeEvent::Failed {
            run_id: run_id.to_string(),
            code: public_failure_code(error),
            message: redact_runtime_text(&error.message),
            recovery: error.code.recovery().to_string(),
        });
    }
}
pub(super) fn public_failure_code(error: &TurnExecutionFailure) -> RuntimeFailureCode {
    if error.message.contains("Required context does not fit")
        || error.message.starts_with("required_context_overflow:")
    {
        return RuntimeFailureCode::RequiredContextOverflow;
    }
    if error.message.contains("Context scope changed")
        || error
            .message
            .contains("Context scope could not be resolved")
        || error
            .message
            .contains("context-scope-changed-after-connect")
    {
        return RuntimeFailureCode::ContextScopeChanged;
    }
    if error.message.contains("A required source is not available")
        || error.message.contains("source-unavailable")
    {
        return RuntimeFailureCode::RequiredContextUnavailable;
    }
    match error.code {
        crate::runtime::contracts::RunFailureCode::ConfigurationError => {
            RuntimeFailureCode::ConfigurationError
        }
        crate::runtime::contracts::RunFailureCode::ChildStartFailed => {
            RuntimeFailureCode::ChildStartFailed
        }
        crate::runtime::contracts::RunFailureCode::RequestTimeout => {
            RuntimeFailureCode::RequestTimeout
        }
        crate::runtime::contracts::RunFailureCode::ProgressTimeout => {
            RuntimeFailureCode::ProgressTimeout
        }
        crate::runtime::contracts::RunFailureCode::TerminalTimeout => {
            RuntimeFailureCode::TerminalTimeout
        }
        crate::runtime::contracts::RunFailureCode::HardTimeout => RuntimeFailureCode::HardTimeout,
        crate::runtime::contracts::RunFailureCode::ChildExited => RuntimeFailureCode::ChildExited,
        crate::runtime::contracts::RunFailureCode::ProtocolError => {
            RuntimeFailureCode::ProtocolError
        }
        crate::runtime::contracts::RunFailureCode::PolicyViolation => {
            RuntimeFailureCode::PolicyViolation
        }
        crate::runtime::contracts::RunFailureCode::ProviderError => {
            RuntimeFailureCode::ProviderError
        }
        crate::runtime::contracts::RunFailureCode::ResponseTooLarge => {
            RuntimeFailureCode::ResponseTooLarge
        }
        crate::runtime::contracts::RunFailureCode::InternalError
        | crate::runtime::contracts::RunFailureCode::AppRestarted => {
            RuntimeFailureCode::InternalError
        }
        crate::runtime::contracts::RunFailureCode::UserCancelled => {
            RuntimeFailureCode::RuntimeError
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_supervised_runtime_run(
    state: &AppState,
    run_id: &str,
    status: &str,
    failure_code: Option<crate::runtime::contracts::RunFailureCode>,
    supervisor_version: Option<&str>,
    last_progress_at: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let changed = transaction
            .execute(
                "UPDATE runtime_runs
                 SET status=?1, error_message=?2, completed_at=?3, failure_code=?4,
                     supervisor_version=?5, last_progress_at=?6
                 WHERE id=?7 AND status='running'",
                params![
                    status,
                    error.map(redact_runtime_text),
                    now_iso(),
                    failure_code.map(crate::runtime::contracts::RunFailureCode::as_str),
                    supervisor_version,
                    last_progress_at,
                    run_id
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Runtime run was already finalized".to_string());
        }
        transaction.commit().map_err(database_error)?;
        Ok(())
    })
}
