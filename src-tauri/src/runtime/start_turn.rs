use std::sync::Arc;

use crate::ipc_contract::RuntimeEvent;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    execute_turn, persistence, redact_runtime_text, register_active_run, validate_identifier,
    validate_start_turn, AppState, RunCancellation, StartTurnInput,
};
use futures_util::FutureExt;

struct TurnDropGuard<'a> {
    state: &'a AppState,
    run_id: &'a str,
    events: &'a dyn RuntimeEventSender,
    armed: bool,
}

impl TurnDropGuard<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TurnDropGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            finalize_unexpected_stop(self.state, self.run_id);
            let _ = self.events.send(RuntimeEvent::Failed {
                run_id: self.run_id.to_string(),
                code: crate::ipc_contract::RuntimeFailureCode::InternalError,
                message: "Conversation runtime stopped unexpectedly".into(),
                recovery: "Retry the request. If it repeats, review the tool audit event.".into(),
            });
        }
    }
}

fn finalize_unexpected_stop(state: &AppState, run_id: &str) {
    let _ = crate::providers::session_store::fail_running_provider_sessions_for_run(
        state,
        run_id,
        crate::ProviderFailureKind::Internal,
    );
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(i64::MAX);
    let _ = state.sqlite_writer.write(|connection| {
        crate::role_routing::repository::record_provider_turn_finish(
            connection, run_id, "failed", None, now_ms,
        )
    });
    let _ = crate::runtime::turns::finish_supervised_runtime_run(
        state,
        run_id,
        "failed",
        Some(crate::runtime::contracts::RunFailureCode::InternalError),
        None,
        None,
        Some("Conversation runtime stopped unexpectedly"),
    );
}

#[tauri::command]
pub(crate) async fn start_turn(
    state: tauri::State<'_, AppState>,
    input: StartTurnInput,
    on_event: tauri::ipc::Channel<RuntimeEvent>,
) -> Result<(), String> {
    validate_start_turn(&input)?;
    validate_identifier(
        input.source_id.as_deref().unwrap_or("none"),
        "turn source id",
    )?;
    let task_mode: String = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT task_mode FROM conversations WHERE id=?1",
                [&input.conversation_id],
                |row| row.get(0),
            )
            .map_err(|_| "Conversation does not exist".to_string())
    })?;
    if task_mode == "conversation" {
        return Err("The legacy conversation runtime was removed pending replacement".into());
    }
    let _ = persistence::audit::record_turn_request(&state, &input);
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, &input.run_id, cancellation.clone())?;
    let event_hub = on_event;
    // Tauri may drop an in-flight command future if its WebView invocation disappears. Keep the
    // durable ledgers terminal even when normal async finalization never gets another poll.
    let mut drop_guard = TurnDropGuard {
        state: &state,
        run_id: &input.run_id,
        events: &event_hub,
        armed: true,
    };
    let execution = std::panic::AssertUnwindSafe(execute_turn(
        &state,
        &input,
        &event_hub,
        cancellation.clone(),
        None,
    ))
    .catch_unwind()
    .await;
    let result = match execution {
        Ok(result) => result,
        Err(_) => {
            let message = "Conversation runtime stopped unexpectedly";
            finalize_unexpected_stop(&state, &input.run_id);
            let _ = event_hub.send(RuntimeEvent::Failed {
                run_id: input.run_id.clone(),
                code: crate::ipc_contract::RuntimeFailureCode::InternalError,
                message: message.into(),
                recovery: "Retry the request. If it repeats, review the tool audit event.".into(),
            });
            Err(crate::TurnExecutionFailure::unsupervised(
                crate::runtime::contracts::RunFailureCode::InternalError,
                message.into(),
            ))
        }
    };
    drop_guard.disarm();
    if let Ok(mut active) = state.active_runs.lock() {
        active.remove(&input.run_id);
    }
    result.map_err(|error| redact_runtime_text(&error.message))
}
