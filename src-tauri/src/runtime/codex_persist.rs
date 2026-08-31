use rusqlite::params;

use crate::ipc_contract::ConversationMessage;
use crate::redact::redact_runtime_text;
use crate::{
    database_error, memory, new_id, now_iso, AppState, CodexTurnOutcome, StartTurnInput,
    TurnExecutionFailure,
};

pub(crate) fn upsert_codex_thread(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &str,
    thread_id: &str,
    model: &str,
    workspace: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO codex_threads(conversation_id, thread_id, model, workspace_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(conversation_id) DO UPDATE SET
               thread_id = excluded.thread_id,
               model = excluded.model,
               workspace_path = excluded.workspace_path,
               updated_at = excluded.updated_at",
            params![conversation_id, thread_id, model, workspace, now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn persist_codex_thread(
    state: &AppState,
    conversation_id: &str,
    thread_id: &str,
    model: &str,
    workspace: &std::path::Path,
) -> Result<(), String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "Codex workspace path is not valid UTF-8".to_string())?;
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        upsert_codex_thread(&transaction, conversation_id, thread_id, model, workspace)?;
        transaction.commit().map_err(database_error)
    })
}

pub(crate) fn persist_codex_success(
    state: &AppState,
    input: &StartTurnInput,
    outcome: &CodexTurnOutcome,
    model: &str,
    workspace: &std::path::Path,
) -> Result<ConversationMessage, String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "Codex workspace path is not valid UTF-8".to_string())?;
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        upsert_codex_thread(
            &transaction,
            &input.conversation_id,
            &outcome.thread_id,
            model,
            workspace,
        )?;
        let message = ConversationMessage {
            id: new_id("message"),
            conversation_id: input.conversation_id.clone(),
            role: "assistant".to_string(),
            content: outcome.content.clone(),
            created_at: now_iso(),
        };
        transaction
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES(?1,?2,'assistant',?3,?4)",
                params![
                    message.id,
                    message.conversation_id,
                    message.content,
                    message.created_at
                ],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at=?1 WHERE id=?2",
                params![message.created_at, input.conversation_id],
            )
            .map_err(database_error)?;
        let changed = transaction
            .execute(
                "UPDATE runtime_runs
             SET status='completed',error_message=NULL,completed_at=?1,failure_code=NULL,
                 supervisor_version=?2,last_progress_at=?3
             WHERE id=?4 AND status='running'",
                params![
                    now_iso(),
                    crate::runtime::contracts::SUPERVISOR_VERSION,
                    outcome.last_progress_at,
                    input.run_id
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Runtime run was already finalized".to_string());
        }
        if memory::control_plane::memory_enabled()
            && transaction
                .execute_batch("SAVEPOINT memory_turn_enqueue")
                .is_ok()
        {
            let memory_now = now_iso();
            let memory_result = transaction
                .query_row(
                    "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                    params![input.run_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(database_error)
                .and_then(|input_message_id| {
                    memory::control_plane::record_completed_turn(
                        &transaction,
                        &input_message_id,
                        &message.id,
                        &memory_now,
                    )
                });
            match memory_result {
                Ok(_) => {
                    if transaction
                        .execute_batch("RELEASE memory_turn_enqueue")
                        .is_ok()
                    {
                        let _ = memory::control_plane::record_decision_event(
                            &transaction,
                            "turn-enqueue",
                            "queued",
                            4,
                            &memory_now,
                        );
                    }
                }
                Err(_) => {
                    let _ = transaction.execute_batch(
                        "ROLLBACK TO memory_turn_enqueue; RELEASE memory_turn_enqueue",
                    );
                    let _ = memory::control_plane::record_decision_event(
                        &transaction,
                        "turn-enqueue",
                        "failed",
                        0,
                        &memory_now,
                    );
                }
            }
        }
        transaction.commit().map_err(database_error)?;
        Ok(message)
    })
}

pub(crate) fn persist_codex_failure(
    state: &AppState,
    input: &StartTurnInput,
    thread_id: Option<&str>,
    model: &str,
    workspace: &std::path::Path,
    error: &TurnExecutionFailure,
    cancelled: bool,
) -> Result<(), String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "Codex workspace path is not valid UTF-8".to_string())?;
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        if let Some(thread_id) = thread_id {
            upsert_codex_thread(
                &transaction,
                &input.conversation_id,
                thread_id,
                model,
                workspace,
            )?;
        }
        let changed = transaction
            .execute(
                "UPDATE runtime_runs
             SET status=?1,error_message=?2,completed_at=?3,failure_code=?4,
                 supervisor_version=?5,last_progress_at=?6
             WHERE id=?7 AND status='running'",
                params![
                    if cancelled { "cancelled" } else { "failed" },
                    redact_runtime_text(&error.message),
                    now_iso(),
                    error.code.as_str(),
                    crate::runtime::contracts::SUPERVISOR_VERSION,
                    error.last_progress_at,
                    input.run_id
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Runtime run was already finalized".to_string());
        }
        transaction.commit().map_err(database_error)
    })
}
